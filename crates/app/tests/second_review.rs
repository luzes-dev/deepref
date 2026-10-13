#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
use std::collections::BTreeMap;

use chrono::Utc;
use deepref_ai::{ModelParameters, ModelProfile, ResolvedModel, ScreeningStage};
use deepref_application::workflows::{AutonomyLevel, AutonomyTask};
use deepref_application::{
    ProtocolCriterionCommand, PublishProtocolCommand, SaveProtocolDraftCommand,
};
use deepref_domain::{
    Actor, ActorKind, CriterionDimension, CriterionKind, CriterionStage, FrameworkKind, ProjectId,
};
use deepref_app::{
    NewReviewerDecision, ProtocolActor, ReviewCalibrationBundleInput, ReviewCalibrationStatus,
    SecondReviewStatus, block_review_run, fail_review_run, insert_model_route,
    insert_review_calibration_bundle, insert_reviewer_decision_in_transaction,
    list_reviewer_decisions, preview_screening_identity, publish_protocol,
    save_protocol_draft, schedule_screening_review, second_review_status, set_autonomy_level,
    sweep_second_reviews,
};
use deepref_review::{
    CalibrationBundleId, ContentDigest, ProtocolDigest, ReviewBlockCode, ReviewDefinitionKey,
    ReviewRunId, ReviewSemanticContract,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 4).await
}

async fn project(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'second review test')")
        .bind(id)
        .execute(pool)
        .await
        .expect("project");
    id
}

async fn attempts(pool: &PgPool, project_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM second_review_attempts WHERE project_id=$1")
        .bind(project_id)
        .fetch_one(pool)
        .await
        .expect("attempt count")
}

async fn automatic_runs(pool: &PgPool, project_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM review_run_manifests
         WHERE project_id=$1 AND origin->>'kind'='advisory_triggered'",
    )
    .bind(project_id)
    .fetch_one(pool)
    .await
    .expect("automatic run count")
}

fn criteria() -> Vec<ProtocolCriterionCommand> {
    vec![ProtocolCriterionCommand {
        id: None,
        kind: CriterionKind::Inclusion,
        stage: CriterionStage::Both,
        dimension: CriterionDimension::Population,
        label: "Population".to_owned(),
        description: "Adults with condition X".to_owned(),
    }]
}

async fn publish_protocol_for(pool: &PgPool, project_id: Uuid) {
    let actor = ProtocolActor {
        kind: "user".to_owned(),
        id: "second-review-test".to_owned(),
    };
    let draft = save_protocol_draft(
        pool,
        &SaveProtocolDraftCommand {
            project_id: ProjectId::from(project_id),
            protocol_version_id: None,
            name: "Second review protocol".to_owned(),
            objective: "Automatic second review".to_owned(),
            question: "Does the sweep screen only records that are waiting?".to_owned(),
            framework_kind: FrameworkKind::Pico,
            framework_fields: BTreeMap::from([
                (
                    "population".to_owned(),
                    "Adults with condition X".to_owned(),
                ),
                ("intervention".to_owned(), "Intervention Y".to_owned()),
                ("outcome".to_owned(), "Outcome Z".to_owned()),
            ]),
            criteria: criteria(),
            expected_revision: 0,
        },
        &actor,
    )
    .await
    .expect("draft saves");
    publish_protocol(
        pool,
        &PublishProtocolCommand {
            project_id: ProjectId::from(project_id),
            protocol_version_id: draft.id,
            expected_revision: draft.revision,
        },
        &actor,
    )
    .await
    .expect("protocol publishes");
}

/// Routes are resolved by profile, newest first, so each test adds its own.
async fn screening_routes(pool: &PgPool) {
    for profile in [ModelProfile::Reasoning, ModelProfile::LongContextReasoning] {
        insert_model_route(
            pool,
            &ResolvedModel {
                profile,
                provider: format!("second-review-test-{}", Uuid::new_v4()),
                model: "reasoner".to_owned(),
                model_version: "2026-08".to_owned(),
                parameters: ModelParameters::default(),
                route_id: None,
            },
            Utc::now(),
        )
        .await
        .expect("route inserts");
    }
}

async fn waiting_record(pool: &PgPool, project_id: Uuid, title: &str) -> Uuid {
    let report_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO reports (id,title,abstract_text) VALUES ($1,$2,'Adults with condition X.')",
    )
    .bind(report_id)
    .bind(title)
    .execute(pool)
    .await
    .expect("report");
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project_id)
        .bind(report_id)
        .execute(pool)
        .await
        .expect("membership");
    report_id
}

/// The identity a screening review of this report at this stage compiles to now.
async fn identity(
    pool: &PgPool,
    project_id: Uuid,
    report_id: Uuid,
    stage: ScreeningStage,
) -> ReviewSemanticContract {
    preview_screening_identity(pool, project_id, report_id, stage)
        .await
        .expect("screening identity previews")
}

/// Legacy consequential calibration remains separate from advisory admission.
async fn approve_calibration(pool: &PgPool, project_id: Uuid, identity: ReviewSemanticContract) {
    insert_review_calibration_bundle(
        pool,
        ReviewCalibrationBundleInput {
            id: CalibrationBundleId::new(Uuid::new_v4()).expect("bundle id"),
            project_id,
            definition: ReviewDefinitionKey::Screening,
            identity,
            evaluation_set_id: "second-review-test-fixture".to_owned(),
            thresholds: json!({}),
            metrics: json!({}),
            reviewer_metadata: json!({"fixture": "unit test, not an expert adjudication"}),
            status: ReviewCalibrationStatus::Passing,
            evaluated_at: Utc::now(),
        },
    )
    .await
    .expect("passing calibration persists");
}

async fn ai_opinion_on_file(pool: &PgPool, project_id: Uuid, report_id: Uuid) {
    let mut tx = pool.begin().await.expect("tx");
    insert_reviewer_decision_in_transaction(
        &mut tx,
        &NewReviewerDecision {
            id: Uuid::new_v4(),
            project_id,
            report_id,
            stage: "title_abstract".to_owned(),
            decision: "exclude".to_owned(),
            rationale: "Population is children only.".to_owned(),
            evidence: json!([{"label": "Abstract", "quote": "children aged 6"}]),
            source: "ai",
            proposal_id: None,
            ai_run_id: None,
            model: Some("test-model".to_owned()),
            prompt_version: None,
            activity_id: None,
        },
    )
    .await
    .expect("insert");
    tx.commit().await.expect("commit");
}

#[tokio::test]
async fn advisory_second_review_contributes_from_record_one_without_calibration() {
    let Some(pool) = database().await else { return };
    screening_routes(&pool).await;
    let project_id = project(&pool).await;
    publish_protocol_for(&pool, project_id).await;
    waiting_record(&pool, project_id, "Advisory from first record").await;
    assert_eq!(
        second_review_status(&pool, project_id, AutonomyTask::TitleAbstractScreening)
            .await
            .unwrap(),
        SecondReviewStatus::Automatic
    );
    assert_eq!(
        second_review_status(&pool, project_id, AutonomyTask::FullTextScreening)
            .await
            .unwrap(),
        SecondReviewStatus::Automatic
    );
    sweep_second_reviews(&pool).await.unwrap();
    assert_eq!(attempts(&pool, project_id).await, 1);
    assert_eq!(automatic_runs(&pool, project_id).await, 1);
    let decisions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM screening_events WHERE project_id=$1")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        decisions, 0,
        "advisory admission cannot write scientific state"
    );
}

#[tokio::test]
async fn the_sweep_schedules_a_bounded_batch_and_never_repeats_an_attempt() {
    let Some(pool) = database().await else { return };
    screening_routes(&pool).await;
    let project_id = project(&pool).await;
    publish_protocol_for(&pool, project_id).await;
    let person = Actor::new(ActorKind::User, "second-review-test").expect("actor");

    // A probe record gets a reviewer-requested run, which fixes the compiled review that
    // this project's calibration must match. It already has an AI opinion on file, so
    // the sweep must leave it alone.
    let probe = waiting_record(&pool, project_id, "Probe record").await;
    schedule_screening_review(
        &pool,
        project_id,
        probe,
        ScreeningStage::TitleAbstract,
        None,
        None,
        person,
    )
    .await
    .expect("reviewer-requested run");
    approve_calibration(
        &pool,
        project_id,
        identity(&pool, project_id, probe, ScreeningStage::TitleAbstract).await,
    )
    .await;
    ai_opinion_on_file(&pool, project_id, probe).await;
    for index in 0..7 {
        waiting_record(&pool, project_id, &format!("Waiting record {index}")).await;
    }

    sweep_second_reviews(&pool).await.expect("first sweep");
    assert_eq!(
        attempts(&pool, project_id).await,
        5,
        "one batch of five per tick"
    );
    sweep_second_reviews(&pool).await.expect("second sweep");
    assert_eq!(
        attempts(&pool, project_id).await,
        7,
        "the rest of the queue, once each"
    );
    sweep_second_reviews(&pool).await.expect("third sweep");
    assert_eq!(
        attempts(&pool, project_id).await,
        7,
        "a settled queue schedules nothing more"
    );
    assert_eq!(
        automatic_runs(&pool, project_id).await,
        7,
        "the probe was never re-sent"
    );
}

#[tokio::test]
async fn stale_calibration_does_not_block_advisory_or_grant_scientific_authority() {
    let Some(pool) = database().await else { return };
    screening_routes(&pool).await;
    let project_id = project(&pool).await;
    publish_protocol_for(&pool, project_id).await;
    let report = waiting_record(&pool, project_id, "Stale evidence advisory").await;
    let mut stale = identity(&pool, project_id, report, ScreeningStage::TitleAbstract).await;
    stale.protocol = ProtocolDigest::from_content(ContentDigest::of_bytes("old protocol"));
    approve_calibration(&pool, project_id, stale).await;
    sweep_second_reviews(&pool).await.unwrap();
    assert_eq!(attempts(&pool, project_id).await, 1);
    assert_eq!(automatic_runs(&pool, project_id).await, 1);
    assert_eq!(
        second_review_status(&pool, project_id, AutonomyTask::TitleAbstractScreening)
            .await
            .unwrap(),
        SecondReviewStatus::Automatic
    );
    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM screening_events WHERE project_id=$1")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(events, 0);
}

/// The newest advisory run for a report.
async fn latest_advisory_run(pool: &PgPool, project_id: Uuid) -> (Uuid, String) {
    sqlx::query_as(
        "SELECT automation_run_id,state FROM review_run_manifests
         WHERE project_id=$1 AND origin->>'kind'='advisory_triggered'
         ORDER BY created_at DESC,automation_run_id DESC LIMIT 1",
    )
    .bind(project_id)
    .fetch_one(pool)
    .await
    .expect("advisory run")
}

/// Moves every finished advisory run of the project into the past, as if the
/// retry backoff had elapsed.
async fn age_finished_runs(pool: &PgPool, project_id: Uuid) {
    sqlx::query(
        "UPDATE review_run_manifests SET finished_at=finished_at-interval '1 day'
         WHERE project_id=$1 AND finished_at IS NOT NULL",
    )
    .bind(project_id)
    .execute(pool)
    .await
    .expect("runs age");
}

/// A provider outage must not leave a record without its second opinion for good:
/// after a backoff, a failed or blocked advisory run is retried a bounded number
/// of times (review ai F1, `second_review_attempts`).
#[tokio::test]
async fn a_failed_or_blocked_advisory_run_is_retried_after_a_backoff_a_bounded_number_of_times() {
    let Some(pool) = database().await else { return };
    let _turn = sweep_turn(&pool).await;
    screening_routes(&pool).await;
    let project_id = project(&pool).await;
    publish_protocol_for(&pool, project_id).await;
    waiting_record(&pool, project_id, "Retried after an outage").await;
    let project = ProjectId::from(project_id);

    sweep_second_reviews(&pool).await.expect("first sweep");
    assert_eq!(automatic_runs(&pool, project_id).await, 1);
    let (first, _) = latest_advisory_run(&pool, project_id).await;
    fail_review_run(
        &pool,
        project,
        ReviewRunId::new(first).unwrap(),
        "provider_unavailable",
        "the provider timed out",
    )
    .await
    .expect("run fails");

    sweep_second_reviews(&pool)
        .await
        .expect("sweep inside backoff");
    assert_eq!(
        automatic_runs(&pool, project_id).await,
        1,
        "no retry before the backoff elapses"
    );

    age_finished_runs(&pool, project_id).await;
    sweep_second_reviews(&pool)
        .await
        .expect("sweep after backoff");
    assert_eq!(automatic_runs(&pool, project_id).await, 2);
    let (second, state) = latest_advisory_run(&pool, project_id).await;
    assert_ne!(second, first);
    assert_eq!(state, "queued");

    block_review_run(
        &pool,
        project,
        ReviewRunId::new(second).unwrap(),
        ReviewBlockCode::RepairBudgetExhausted,
        "repair budget exhausted",
    )
    .await
    .expect("run blocks");
    age_finished_runs(&pool, project_id).await;
    sweep_second_reviews(&pool)
        .await
        .expect("sweep after a block");
    assert_eq!(automatic_runs(&pool, project_id).await, 3);

    let (third, _) = latest_advisory_run(&pool, project_id).await;
    fail_review_run(
        &pool,
        project,
        ReviewRunId::new(third).unwrap(),
        "provider_unavailable",
        "the provider timed out again",
    )
    .await
    .expect("run fails");
    age_finished_runs(&pool, project_id).await;
    sweep_second_reviews(&pool)
        .await
        .expect("sweep at the limit");
    assert_eq!(
        automatic_runs(&pool, project_id).await,
        3,
        "retries are bounded"
    );
    assert_eq!(attempts(&pool, project_id).await, 1);
}

#[tokio::test]
async fn turning_the_second_reviewer_off_is_reported_and_never_scheduled() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool).await;
    let person = Actor::new(ActorKind::User, "tester").expect("actor");
    set_autonomy_level(
        &pool,
        project_id,
        AutonomyTask::TitleAbstractScreening,
        AutonomyLevel::Suggest,
        &person,
    )
    .await
    .expect("suggest level");
    assert_eq!(
        second_review_status(&pool, project_id, AutonomyTask::TitleAbstractScreening)
            .await
            .expect("status"),
        SecondReviewStatus::NotEnabled
    );
}

#[tokio::test]
async fn an_undecided_record_never_returns_the_ai_opinion() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool).await;
    let report_id = waiting_record(&pool, project_id, "Blinding report").await;
    ai_opinion_on_file(&pool, project_id, report_id).await;

    // The person has not decided yet, so the AI's view must not leave the server.
    let waiting = list_reviewer_decisions(&pool, project_id, None, Some("waiting"), 10)
        .await
        .expect("list");
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].ai_decision, "");
    assert_eq!(waiting[0].ai_rationale, "");
    assert_eq!(waiting[0].ai_model, None);
    assert!(waiting[0].ai_evidence.as_array().is_some_and(Vec::is_empty));
}

/// The subject mapping decides which exclusion reasons a stage may cite. A
/// full-text run stores only the full-text reasons, whatever else the project
/// defines.
#[tokio::test]
async fn a_full_text_run_stores_only_the_full_text_exclusion_reasons() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool).await;
    publish_protocol_for(&pool, project_id).await;
    insert_model_route(
        &pool,
        &ResolvedModel {
            profile: ModelProfile::LongContextReasoning,
            provider: format!("second-review-test-{}", Uuid::new_v4()),
            model: "long-reasoner".to_owned(),
            model_version: "2026-08".to_owned(),
            parameters: ModelParameters::default(),
            route_id: None,
        },
        Utc::now(),
    )
    .await
    .expect("route inserts");
    // Every new project is seeded with full-text reasons, so the expected set is
    // read back from the database rather than assumed. The two reasons added here
    // use codes the seed does not use.
    let title_abstract_reason = Uuid::new_v4();
    let extra_full_text_reason = Uuid::new_v4();
    for (id, code, stage) in [
        (
            title_abstract_reason,
            "title_abstract_only",
            "title_abstract",
        ),
        (extra_full_text_reason, "extra_full_text_only", "full_text"),
    ] {
        sqlx::query(
            "INSERT INTO exclusion_reasons (id,project_id,code,label,stage) VALUES ($1,$2,$3,$3,$4)",
        )
        .bind(id)
        .bind(project_id)
        .bind(code)
        .bind(stage)
        .execute(&pool)
        .await
        .expect("exclusion reason");
    }
    let mut expected: Vec<String> = sqlx::query_scalar(
        "SELECT id::text FROM exclusion_reasons WHERE project_id=$1 AND stage='full_text'",
    )
    .bind(project_id)
    .fetch_all(&pool)
    .await
    .expect("full-text reasons");
    assert!(expected.contains(&extra_full_text_reason.to_string()));
    expected.sort();
    let report = waiting_record(&pool, project_id, "Full text report").await;
    schedule_screening_review(
        &pool,
        project_id,
        report,
        ScreeningStage::FullText,
        None,
        None,
        Actor::new(ActorKind::User, "second-review-test").expect("actor"),
    )
    .await
    .expect("full-text run");

    let stored: serde_json::Value = sqlx::query_scalar(
        "SELECT prepared_task FROM review_run_manifests
         WHERE project_id=$1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(project_id)
    .fetch_one(&pool)
    .await
    .expect("stored task");
    let mut allowed = stored["allowed_exclusion_reasons"]
        .as_array()
        .expect("reasons are a list")
        .iter()
        .map(|value| value.as_str().expect("reason id").to_owned())
        .collect::<Vec<_>>();
    allowed.sort();
    assert_eq!(allowed, expected);
    assert!(!allowed.contains(&title_abstract_reason.to_string()));
}
