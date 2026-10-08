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
use deepref_postgres::{
    NewReviewerDecision, ProtocolActor, ReviewCalibrationBundleInput, ReviewCalibrationStatus,
    SecondReviewStatus, insert_model_route, insert_review_calibration_bundle,
    insert_reviewer_decision_in_transaction, list_reviewer_decisions, migrate, publish_protocol,
    save_protocol_draft, schedule_screening_review, second_review_status, set_autonomy_level,
    sweep_second_reviews,
};
use deepref_review::{CalibrationBundleId, ReviewDefinitionKey, worker::ReviewHash};
use serde_json::json;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::sync::Mutex;
use uuid::Uuid;

/// A sweep looks at every calibrated project in the database, so the tests that
/// run one take turns. Each still asserts only on the attempts of its own project.
static SWEEP_TURN: Mutex<()> = Mutex::const_new(());

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .ok()?;
    migrate(&pool).await.ok()?;
    Some(pool)
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
         WHERE project_id=$1 AND origin->>'kind'='automation_triggered'",
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

async fn screening_route(pool: &PgPool) {
    insert_model_route(
        pool,
        &ResolvedModel {
            profile: ModelProfile::Reasoning,
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

/// The compiled review hash of this project, read from a run that was actually scheduled.
async fn compiled_hash(pool: &PgPool, project_id: Uuid) -> String {
    sqlx::query_scalar(
        "SELECT semantic_bundle_hash FROM review_run_manifests
         WHERE project_id=$1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(project_id)
    .fetch_one(pool)
    .await
    .expect("a compiled manifest")
}

async fn approve_calibration(pool: &PgPool, project_id: Uuid, hash: String) {
    insert_review_calibration_bundle(
        pool,
        ReviewCalibrationBundleInput {
            id: CalibrationBundleId::new(Uuid::new_v4()).expect("bundle id"),
            project_id,
            definition: ReviewDefinitionKey::Screening,
            semantic_bundle_hash: ReviewHash::parse(hash).expect("hash"),
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
async fn automatic_second_review_needs_an_approved_calibration_and_says_so() {
    let Some(pool) = database().await else { return };
    let _turn = SWEEP_TURN.lock().await;
    let project_id = project(&pool).await;
    publish_protocol_for(&pool, project_id).await;
    screening_route(&pool).await;
    let report_id = waiting_record(&pool, project_id, "Waiting without calibration").await;

    // Second reviewer is the default for both screening stages, but nothing
    // calibrated admits it yet, so the status says why it does not run.
    assert_eq!(
        second_review_status(&pool, project_id, AutonomyTask::TitleAbstractScreening)
            .await
            .expect("status"),
        SecondReviewStatus::NeedsCalibration
    );
    assert_eq!(
        second_review_status(&pool, project_id, AutonomyTask::FullTextScreening)
            .await
            .expect("status"),
        SecondReviewStatus::NeedsCalibration
    );
    sweep_second_reviews(&pool).await.expect("sweep");
    assert_eq!(attempts(&pool, project_id).await, 0);
    assert_eq!(automatic_runs(&pool, project_id).await, 0);

    // A reviewer-requested run fixes the compiled hash; an approved calibration for
    // exactly that review then turns the status automatic.
    schedule_screening_review(
        &pool,
        project_id,
        report_id,
        ScreeningStage::TitleAbstract,
        None,
        None,
        Actor::new(ActorKind::User, "second-review-test").expect("actor"),
    )
    .await
    .expect("reviewer-requested run");
    approve_calibration(&pool, project_id, compiled_hash(&pool, project_id).await).await;
    assert_eq!(
        second_review_status(&pool, project_id, AutonomyTask::TitleAbstractScreening)
            .await
            .expect("status"),
        SecondReviewStatus::Automatic
    );
}

#[tokio::test]
async fn the_sweep_schedules_a_bounded_batch_and_never_repeats_an_attempt() {
    let Some(pool) = database().await else { return };
    let _turn = SWEEP_TURN.lock().await;
    let project_id = project(&pool).await;
    publish_protocol_for(&pool, project_id).await;
    screening_route(&pool).await;
    let person = Actor::new(ActorKind::User, "second-review-test").expect("actor");

    // A probe record gets a reviewer-requested run, which fixes the compiled hash that
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
    approve_calibration(&pool, project_id, compiled_hash(&pool, project_id).await).await;
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
async fn a_calibration_for_a_different_review_releases_the_attempt_and_stops() {
    let Some(pool) = database().await else { return };
    let _turn = SWEEP_TURN.lock().await;
    let project_id = project(&pool).await;
    publish_protocol_for(&pool, project_id).await;
    screening_route(&pool).await;
    waiting_record(&pool, project_id, "Waiting under a stale calibration").await;
    approve_calibration(&pool, project_id, "b".repeat(64)).await;

    sweep_second_reviews(&pool).await.expect("sweep");
    assert_eq!(
        attempts(&pool, project_id).await,
        0,
        "the claim is released for a later tick"
    );
    assert_eq!(
        automatic_runs(&pool, project_id).await,
        0,
        "nothing was admitted"
    );
    assert_eq!(
        second_review_status(&pool, project_id, AutonomyTask::TitleAbstractScreening)
            .await
            .expect("status"),
        SecondReviewStatus::CalibrationStale,
        "the status must not read automatic while the gate refuses the calibration"
    );
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
