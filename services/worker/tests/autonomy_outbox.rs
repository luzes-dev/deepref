#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Autonomy for a completed review is committed with the proposal, so a failure
//! (or a crash) right after finalization cannot lose the AI second opinion
//! (review ai F13).
use deepref_ai::{
    AiFuture, AiGateway, CompletionRequest, GatewayCompletion, ModelParameters, ModelProfile,
    ResolvedModel, ScreeningStage,
};
use deepref_application::{
    ProtocolCriterionCommand, PublishProtocolCommand, SaveProtocolDraftCommand,
};
use deepref_domain::{
    Actor, ActorKind, CriterionDimension, CriterionKind, CriterionStage, FrameworkKind,
};
use deepref_postgres::{
    ProtocolActor, claim_job, complete_job, insert_model_route, migrate, publish_protocol,
    save_protocol_draft, schedule_screening_review,
};
use deepref_worker::{delivery::DeliveryAction, processor::handle_job_with_documents_owned_and_ai};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use uuid::Uuid;

/// Answers every screening prompt with a grounded exclusion.
struct ExcludingGateway;
impl AiGateway for ExcludingGateway {
    fn complete<'a>(&'a self, request: CompletionRequest) -> AiFuture<'a, GatewayCompletion> {
        Box::pin(async move {
            let prompt: Value = serde_json::Deserializer::from_str(&request.user_prompt)
                .into_iter()
                .next()
                .unwrap()
                .unwrap();
            let prompt = prompt.get("source").unwrap_or(&prompt);
            let evidence: Vec<Value> = prompt["allowed_evidence"]
                .as_object()
                .unwrap()
                .values()
                .cloned()
                .collect();
            let input = &prompt["input"];
            let criteria: Vec<_> = input["criteria"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| {
                    json!({
                        "criterion_id": c["id"],
                        "judgment": "does_not_meet",
                        "rationale": "The title and abstract identify a child population.",
                        "evidence": evidence,
                    })
                })
                .collect();
            Ok(GatewayCompletion {
                output_json: json!({
                    "report_id": input["report_id"], "stage": input["stage"],
                    "protocol_version_id": input["protocol_version_id"],
                    "expected_revision": input["expected_revision"],
                    "criteria": criteria,
                    "suggested_decision": {"kind": "exclude", "exclusion_reason_id": null},
                    "uncertainties": [],
                })
                .to_string(),
                input_tokens: 1,
                output_tokens: 1,
                cost_micros: None,
                served_model: Some("fixture-served-model".into()),
                system_fingerprint: Some("fixture-system-v1".into()),
            })
        })
    }
}

async fn publish(pool: &sqlx::PgPool, project: Uuid) {
    let actor = ProtocolActor {
        kind: "user".into(),
        id: "owner".into(),
    };
    let draft = save_protocol_draft(
        pool,
        &SaveProtocolDraftCommand {
            project_id: project.into(),
            protocol_version_id: None,
            name: "Population".into(),
            objective: "Screen adults".into(),
            question: "Adults with condition X?".into(),
            framework_kind: FrameworkKind::Pico,
            framework_fields: BTreeMap::from([
                ("population".into(), "Adults".into()),
                ("intervention".into(), "Y".into()),
                ("outcome".into(), "Z".into()),
            ]),
            criteria: vec![ProtocolCriterionCommand {
                id: None,
                kind: CriterionKind::Inclusion,
                stage: CriterionStage::Both,
                dimension: CriterionDimension::Population,
                label: "Adults".into(),
                description: "Adults with condition X".into(),
            }],
            expected_revision: 0,
        },
        &actor,
    )
    .await
    .unwrap();
    publish_protocol(
        pool,
        &PublishProtocolCommand {
            project_id: project.into(),
            protocol_version_id: draft.id,
            expected_revision: draft.revision,
        },
        &actor,
    )
    .await
    .unwrap();
}

async fn reviewer_decisions(pool: &sqlx::PgPool, project: Uuid, report: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM ai_reviewer_decisions WHERE project_id=$1 AND report_id=$2",
    )
    .bind(project)
    .bind(report)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_second_opinion_lost_after_finalization_is_recorded_by_the_reconciler() {
    let Some(pool) =
        deepref_postgres::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
    else {
        return;
    };
    migrate(&pool).await.unwrap();
    let project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'autonomy outbox test')")
        .bind(project)
        .execute(&pool)
        .await
        .unwrap();
    insert_model_route(
        &pool,
        &ResolvedModel {
            profile: ModelProfile::Reasoning,
            provider: "autonomy-outbox-fixture".into(),
            model: "fixture".into(),
            model_version: "immutable-test-v1".into(),
            parameters: ModelParameters::default(),
            route_id: None,
        },
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    publish(&pool, project).await;
    let report = Uuid::new_v4();
    sqlx::query("INSERT INTO reports(id,title,abstract_text) VALUES($1,$2,$3)")
        .bind(report)
        .bind("Exclude children")
        .bind("This randomized study involves children under age twelve with condition X and intervention Y.")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
        .bind(project)
        .bind(report)
        .execute(&pool)
        .await
        .unwrap();

    // The post-commit autonomy step fails, as if the worker stopped right after
    // the review run's finalization committed.
    sqlx::query(
        "CREATE FUNCTION fail_reviewer_decision() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'simulated stop after finalization'; END $$",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_reviewer_decision BEFORE INSERT ON ai_reviewer_decisions
         FOR EACH ROW EXECUTE FUNCTION fail_reviewer_decision()",
    )
    .execute(&pool)
    .await
    .unwrap();

    let reviewer = Actor::new(ActorKind::User, "reviewer").unwrap();
    let run = schedule_screening_review(
        &pool,
        project,
        report,
        ScreeningStage::TitleAbstract,
        None,
        None,
        reviewer,
    )
    .await
    .unwrap();
    let gateway: Arc<dyn AiGateway> = Arc::new(ExcludingGateway);
    let worker = "autonomy-outbox-test";
    for _ in 0..10 {
        let Some(job) = claim_job(&pool, worker, Duration::from_secs(60))
            .await
            .unwrap()
        else {
            break;
        };
        let lease = job.lease(worker);
        let action = handle_job_with_documents_owned_and_ai(
            pool.clone(),
            &job,
            &lease,
            Duration::from_secs(60),
            None,
            None,
            gateway.clone(),
        )
        .await
        .unwrap();
        assert_eq!(action, DeliveryAction::Ack);
        assert!(complete_job(&pool, &lease).await.unwrap());
    }
    let state: String = sqlx::query_scalar(
        "SELECT state FROM review_run_manifests WHERE project_id=$1 AND automation_run_id=$2",
    )
    .bind(project)
    .bind(run.id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, "completed");
    assert_eq!(reviewer_decisions(&pool, project, report).await, 0);

    sqlx::query("DROP TRIGGER fail_reviewer_decision ON ai_reviewer_decisions")
        .execute(&pool)
        .await
        .unwrap();
    deepref_worker::second_review::sweep(&pool).await;
    assert_eq!(
        reviewer_decisions(&pool, project, report).await,
        1,
        "the committed autonomy is applied once the failure clears"
    );
    deepref_worker::second_review::sweep(&pool).await;
    assert_eq!(reviewer_decisions(&pool, project, report).await, 1);
}
