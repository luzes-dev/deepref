#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Production compiled worker artifacts must satisfy the deterministic routing
//! policy before a disposition can change queue visibility.
use deepref_ai::{
    AiFuture, AiGateway, CompletionRequest, GatewayCompletion, ModelParameters, ModelProfile,
    ResolvedModel,
};
use deepref_application::{
    ProtocolCriterionCommand, PublishProtocolCommand, SaveProtocolDraftCommand,
};
use deepref_domain::{
    Actor, ActorKind, CriterionDimension, CriterionKind, CriterionStage, FrameworkKind,
};
use deepref_postgres::{
    ProtocolActor, claim_job, complete_job, insert_model_route, migrate, publish_protocol,
    save_protocol_draft, start_ai_first_cohort, sweep_ai_first,
};
use deepref_worker::{delivery::DeliveryAction, processor::handle_job_with_documents_owned_and_ai};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use uuid::Uuid;

struct GroundedGateway;
impl AiGateway for GroundedGateway {
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
            let include = input["title"].as_str().unwrap().contains("Retain");
            let criteria: Vec<_> = input["criteria"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| {
                    json!({
                        "criterion_id": c["id"],
                        "judgment": if include {"meets"} else {"does_not_meet"},
                        "rationale": "The title and abstract identify the population.",
                        "evidence": evidence,
                    })
                })
                .collect();
            Ok(GatewayCompletion {
                output_json: json!({
                    "report_id": input["report_id"], "stage": input["stage"],
                    "protocol_version_id": input["protocol_version_id"], "expected_revision": input["expected_revision"],
                    "criteria": criteria, "suggested_decision": {"kind": if include {"include"} else {"exclude"}, "exclusion_reason_id": null},
                    "uncertainties": [],
                }).to_string(), input_tokens: 1, output_tokens: 1, cost_micros: None,
                served_model: Some("fixture-served-model".into()), system_fingerprint: Some("fixture-system-v1".into()),
            })
        })
    }
}

#[tokio::test]
async fn compiled_worker_quarantines_only_eligible_exclusions_without_scientific_writes() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        return;
    };
    let pool: PgPool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .unwrap();
    migrate(&pool).await.unwrap();
    sqlx::query("UPDATE jobs SET state='dead',last_error='parked by compiled AI-first test',completed_at=now() WHERE state IN ('queued','running')")
        .execute(&pool).await.unwrap();
    let project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'compiled AI first test')")
        .bind(project)
        .execute(&pool)
        .await
        .unwrap();
    insert_model_route(
        &pool,
        &ResolvedModel {
            profile: ModelProfile::Reasoning,
            provider: "compiled-ai-first-fixture".into(),
            model: "fixture".into(),
            model_version: "immutable-test-v1".into(),
            parameters: ModelParameters::default(),
            route_id: None,
        },
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    let actor = ProtocolActor {
        kind: "user".into(),
        id: "owner".into(),
    };
    let draft = save_protocol_draft(
        &pool,
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
        &pool,
        &PublishProtocolCommand {
            project_id: project.into(),
            protocol_version_id: draft.id,
            expected_revision: draft.revision,
        },
        &actor,
    )
    .await
    .unwrap();
    let mut reports = vec![];
    for (title, abstract_text) in [
        (
            "Exclude children",
            Some(
                "This randomized study involves children under age twelve with condition X and intervention Y.",
            ),
        ),
        ("Exclude incomplete", None),
        (
            "Retain adults",
            Some(
                "This randomized study involves adults over age eighteen with condition X and intervention Y.",
            ),
        ),
    ] {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO reports(id,title,abstract_text) VALUES($1,$2,$3)")
            .bind(id)
            .bind(title)
            .bind(abstract_text)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
            .bind(project)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        reports.push(id);
    }
    let owner = Actor::new(ActorKind::User, "owner").unwrap();
    let cohort = start_ai_first_cohort(&pool, project, 95, false, &owner)
        .await
        .unwrap();
    assert_eq!(sweep_ai_first(&pool).await.unwrap(), 3);
    let gateway: Arc<dyn AiGateway> = Arc::new(GroundedGateway);
    let worker = "compiled-ai-first-test";
    for _ in 0..10 {
        let Some(job) = claim_job(&pool, worker, Duration::from_secs(60))
            .await
            .unwrap()
        else {
            break;
        };
        assert_eq!(job.project_id.as_uuid(), project);
        let action = handle_job_with_documents_owned_and_ai(
            pool.clone(),
            &job,
            worker,
            Duration::from_secs(60),
            None,
            None,
            gateway.clone(),
        )
        .await
        .unwrap();
        assert_eq!(action, DeliveryAction::Ack);
        assert!(complete_job(&pool, worker, job.id).await.unwrap());
    }
    let states: Vec<String> =
        sqlx::query_scalar("SELECT state FROM review_run_manifests WHERE project_id=$1")
            .bind(project)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(states, vec!["completed"; 3]);
    sweep_ai_first(&pool).await.unwrap();
    let quarantined: Vec<Uuid> = sqlx::query_scalar(
        "SELECT report_id FROM ai_screening_dispositions WHERE cohort_id=$1 AND voided_at IS NULL",
    )
    .bind(cohort)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(quarantined, vec![reports[0]]);
    let decisions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM screening_events WHERE project_id=$1")
            .bind(project)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        decisions, 0,
        "quarantine never writes a scientific exclusion"
    );
    let visible: bool = sqlx::query_scalar("SELECT ai_first_run_visible($1,automation_run_id) FROM review_run_manifests WHERE project_id=$1 LIMIT 1")
        .bind(project).fetch_one(&pool).await.unwrap();
    assert!(
        !visible,
        "conditional workflow topology remains blind during reference collection"
    );
    let exposure: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ai_opinion_exposures WHERE project_id=$1")
            .bind(project)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        exposure, 0,
        "worker results are withheld, not delivered to audit reviewers"
    );
    let status: String = sqlx::query_scalar("SELECT status FROM ai_screening_cohorts WHERE id=$1")
        .bind(cohort)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "closed");
    let export = deepref_postgres::load_audit_export_rows(&pool, project, 10_000)
        .await
        .unwrap();
    assert!(
        !export.iter().any(|row| matches!(
            row.event_type.as_str(),
            "ai_run_snapshot"
                | "ai_proposal_snapshot"
                | "automation_run_snapshot"
                | "automation_step_snapshot"
                | "review_run_manifest"
                | "review_step_attempt"
                | "review_artifact"
        )),
        "active audit exports must hide model-call counts and conditional workflow topology as well as verdicts"
    );
    // Reports imported later wait for a separately approved cohort.
    let later = Uuid::new_v4();
    sqlx::query("INSERT INTO reports(id,title) VALUES($1,'Later import')")
        .bind(later)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
        .bind(project)
        .bind(later)
        .execute(&pool)
        .await
        .unwrap();
    let members: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ai_screening_cohort_members WHERE cohort_id=$1")
            .bind(cohort)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(members, 3);
    // Changed model identity releases quarantine automatically.
    insert_model_route(
        &pool,
        &ResolvedModel {
            profile: ModelProfile::Reasoning,
            provider: "compiled-ai-first-fixture".into(),
            model: "fixture".into(),
            model_version: "immutable-test-v2".into(),
            parameters: ModelParameters::default(),
            route_id: None,
        },
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    sweep_ai_first(&pool).await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM ai_screening_cohorts WHERE id=$1")
        .bind(cohort)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "invalidated");
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ai_screening_dispositions WHERE cohort_id=$1 AND voided_at IS NULL",
    )
    .bind(cohort)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(active, 0);
    // A zero budget leaves untouched records to humans and closes partial coverage.
    let partial = start_ai_first_cohort(&pool, project, 98, false, &owner)
        .await
        .unwrap();
    deepref_postgres::set_ai_budget(&pool, project, 0)
        .await
        .unwrap();
    sweep_ai_first(&pool).await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM ai_screening_cohorts WHERE id=$1")
        .bind(partial)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "closed");
    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_screening_cohort_members WHERE cohort_id=$1 AND review_run_id IS NOT NULL")
        .bind(partial).fetch_one(&pool).await.unwrap();
    assert_eq!(runs, 0);
}
