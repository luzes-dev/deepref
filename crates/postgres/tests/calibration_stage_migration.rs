#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
//! Migration 0050 on a database that already holds a legacy calibration bundle, and
//! the schema rules that keep the legacy shape from being written again.

use std::collections::BTreeMap;

use chrono::Utc;
use deepref_ai::{ModelParameters, ModelProfile, ResolvedModel};
use deepref_application::workflows::AutonomyTask;
use deepref_application::{ProtocolCriterionCommand, SaveProtocolDraftCommand};
use deepref_domain::{
    Actor, ActorKind, CriterionDimension, CriterionKind, CriterionStage, FrameworkKind, ProjectId,
    ProtocolVersionId, ReportId, ScreeningStage,
};
use deepref_postgres::{
    CalibrationRefusal, MIGRATOR, PostgresReviewError, PostgresReviewScheduler, ProtocolActor,
    ReviewPreparationError, SecondReviewStatus, get_ai_screening_target, get_published_protocol,
    insert_model_route, migrate, save_protocol_draft, second_review_status, sweep_second_reviews,
};
use deepref_review::{
    CalibrationBundleId, ReviewDefinitionKey, ReviewOrigin, ReviewRunSnapshot, ReviewScheduler,
    ReviewSubject, ScheduleReviewRun,
};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool, Row, migrate::Migrator, postgres::PgPoolOptions};
use uuid::Uuid;

/// The migration that introduces stage-scoped calibration. Every earlier migration
/// is applied before the legacy rows are written.
const STAGE_SCOPED_CALIBRATION: i64 = 50;

fn database_url() -> Option<String> {
    std::env::var("DATABASE_URL").ok()
}

/// The same server and credentials, pointed at another database.
fn url_for_database(url: &str, database: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (url, None),
    };
    let (server, _current_database) = base
        .rsplit_once('/')
        .expect("DATABASE_URL names a database");
    match query {
        Some(query) => format!("{server}/{database}?{query}"),
        None => format!("{server}/{database}"),
    }
}

async fn pool_for(url: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(4)
        .connect(url)
        .await
        .expect("database is reachable")
}

async fn publish_protocol_for(pool: &PgPool, project_id: ProjectId) {
    let actor = ProtocolActor {
        kind: "user".to_owned(),
        id: "migration-test".to_owned(),
    };
    let draft = save_protocol_draft(
        pool,
        &SaveProtocolDraftCommand {
            project_id,
            protocol_version_id: None,
            name: "Migration protocol".to_owned(),
            objective: "Legacy calibration".to_owned(),
            question: "Does a legacy bundle admit automation?".to_owned(),
            framework_kind: FrameworkKind::Pico,
            framework_fields: BTreeMap::from([
                (
                    "population".to_owned(),
                    "Adults with condition X".to_owned(),
                ),
                ("intervention".to_owned(), "Intervention Y".to_owned()),
                ("outcome".to_owned(), "Outcome Z".to_owned()),
            ]),
            criteria: vec![ProtocolCriterionCommand {
                id: None,
                kind: CriterionKind::Inclusion,
                stage: CriterionStage::Both,
                dimension: CriterionDimension::Population,
                label: "Population".to_owned(),
                description: "Adults with condition X".to_owned(),
            }],
            expected_revision: 0,
        },
        &actor,
    )
    .await
    .expect("draft saves");
    // Historical schema fixtures cannot call current publication hooks that
    // require later tables. Seed the already validated draft's published state.
    sqlx::query("UPDATE protocol_versions SET status='published',published_at=now(),published_by_kind='user',published_by_id='migration-test' WHERE id=$1")
        .bind(draft.id).execute(pool).await.expect("historical protocol publishes");
}

/// A project with a published protocol, one report to screen, and a Reasoning route.
async fn screening_fixture(pool: &PgPool) -> (ProjectId, Uuid) {
    let project_id = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'legacy calibration')")
        .bind(project_id.as_uuid())
        .execute(pool)
        .await
        .expect("project inserts");
    publish_protocol_for(pool, project_id).await;
    let report_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO reports (id,title,abstract_text) VALUES ($1,'Waiting record','Adults with condition X.')",
    )
    .bind(report_id)
    .execute(pool)
    .await
    .expect("report inserts");
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project_id.as_uuid())
        .bind(report_id)
        .execute(pool)
        .await
        .expect("membership inserts");
    insert_model_route(
        pool,
        &ResolvedModel {
            profile: ModelProfile::Reasoning,
            provider: format!("legacy-test-{}", Uuid::new_v4()),
            model: "reasoner".to_owned(),
            model_version: "2026-08".to_owned(),
            parameters: ModelParameters::default(),
            route_id: None,
        },
        Utc::now(),
    )
    .await
    .expect("route inserts");
    (project_id, report_id)
}

/// A title/abstract screening review scheduled against `bundle`, the way the
/// automation sweep schedules one.
async fn automated_title_abstract_run(
    pool: &PgPool,
    project_id: ProjectId,
    report_id: Uuid,
    bundle: CalibrationBundleId,
) -> Result<ReviewRunSnapshot, ReviewPreparationError> {
    let protocol = get_published_protocol(pool, project_id.as_uuid())
        .await
        .expect("published protocol");
    let target = get_ai_screening_target(pool, project_id.as_uuid(), report_id)
        .await
        .expect("screening target");
    PostgresReviewScheduler::new(pool)
        .schedule(ScheduleReviewRun {
            project_id,
            definition: ReviewDefinitionKey::Screening,
            subject: ReviewSubject::Screening {
                report_id: ReportId::new(report_id),
                stage: ScreeningStage::TitleAbstract,
                protocol_version_id: ProtocolVersionId::new(protocol.id),
                expected_revision: target.expected_revision,
            },
            origin: ReviewOrigin::AutomationTriggered {
                calibration_bundle_id: bundle,
            },
            actor: Actor::new(ActorKind::Automation, "legacy-test").expect("actor"),
        })
        .await
}

/// Inserts one bundle with explicit identity columns, so each schema rule can be
/// exercised on its own.
async fn insert_raw_bundle(
    pool: &PgPool,
    project_id: Uuid,
    definition: &str,
    stage: Option<&str>,
    scheme: i32,
    snapshot: Option<Value>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO review_calibration_bundles
         (id,project_id,definition_key,stage,identity_scheme,identity_snapshot,
          semantic_bundle_hash,evaluation_set_id,thresholds,metrics,reviewer_metadata,
          status,evaluated_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,'raw-test',$8,$9,$10,'passing',now())",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(definition)
    .bind(stage)
    .bind(scheme)
    .bind(snapshot)
    .bind("e".repeat(64))
    .bind(json!({}))
    .bind(json!({}))
    .bind(json!({"fixture": "raw schema test"}))
    .execute(pool)
    .await
    .map(|_| ())
}

/// A snapshot shaped for a bundle of `definition` at `stage` under `scheme`.
fn snapshot(scheme: i32, definition: &str, stage: Option<&str>) -> Value {
    json!({
        "scheme": scheme,
        "definition": definition,
        "stage": stage,
        "components": {"protocol": "f".repeat(64)},
    })
}

fn rejection_message(error: sqlx::Error) -> String {
    match error {
        sqlx::Error::Database(database) => database.message().to_owned(),
        other => panic!("expected a database rejection, got: {other}"),
    }
}

#[tokio::test]
async fn legacy_calibration_bundles_keep_scheme_one_and_never_admit_automation() {
    let Some(url) = database_url() else { return };
    let database = format!("deepref_pr2_legacy_{}", Uuid::new_v4().simple());
    let mut maintenance = PgConnection::connect(&url_for_database(&url, "postgres"))
        .await
        .expect("maintenance connection");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE DATABASE \"{database}\""
    )))
    .execute(&mut maintenance)
    .await
    .expect("legacy database is created");

    // The assertions run in their own task, so the temporary database is dropped
    // even when one of them fails.
    let legacy_url = url_for_database(&url, &database);
    let outcome = tokio::spawn(async move { upgrade_legacy_database(&legacy_url).await }).await;

    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE \"{database}\" WITH (FORCE)"
    )))
    .execute(&mut maintenance)
    .await
    .expect("legacy database is dropped");
    match outcome {
        Ok(()) => {}
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        Err(error) => panic!("legacy migration check was cancelled: {error}"),
    }
}

/// Migrates a database to 0048, writes a legacy bundle, and runs the rest.
async fn upgrade_legacy_database(legacy_url: &str) {
    let legacy = pool_for(legacy_url).await;
    let through_0048 = Migrator::with_migrations(
        MIGRATOR
            .iter()
            .filter(|migration| migration.version < STAGE_SCOPED_CALIBRATION)
            .cloned()
            .collect(),
    );
    through_0048
        .run(&legacy)
        .await
        .expect("migrations before 0050 apply");

    // A bundle as migration 0022 wrote it: one aggregate hash, no stage, no identity.
    let (project_id, report_id) = screening_fixture(&legacy).await;
    let legacy_bundle = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO review_calibration_bundles
         (id,project_id,definition_key,semantic_bundle_hash,evaluation_set_id,
          thresholds,metrics,reviewer_metadata,status,evaluated_at)
         VALUES ($1,$2,'screening',$3,'legacy-expert-v1',$4,$5,$6,'passing',now())",
    )
    .bind(legacy_bundle)
    .bind(project_id.as_uuid())
    .bind("a".repeat(64))
    .bind(json!({"precision": 0.99}))
    .bind(json!({"precision": 1.0}))
    .bind(json!({"reviewer_ids": ["expert-1"]}))
    .execute(&legacy)
    .await
    .expect("legacy bundle inserts under the 0022 shape");

    // Migration 0050 and everything after it run on top of that data.
    migrate(&legacy)
        .await
        .expect("migration 0050 applies to legacy rows");

    let row = sqlx::query(
        "SELECT identity_scheme, stage, identity_snapshot IS NULL AS no_snapshot
         FROM review_calibration_bundles WHERE id=$1",
    )
    .bind(legacy_bundle)
    .fetch_one(&legacy)
    .await
    .expect("legacy bundle remains");
    assert_eq!(row.get::<i32, _>("identity_scheme"), 1);
    assert_eq!(row.get::<Option<String>, _>("stage"), None);
    assert!(row.get::<bool, _>("no_snapshot"));

    let update = sqlx::query("UPDATE review_calibration_bundles SET status='failed' WHERE id=$1")
        .bind(legacy_bundle)
        .execute(&legacy)
        .await
        .expect_err("the immutability trigger still rejects updates");
    assert!(rejection_message(update).contains("immutable"));

    // Admission names the legacy identity rather than admitting or guessing a stage.
    let refusal = automated_title_abstract_run(
        &legacy,
        project_id,
        report_id,
        CalibrationBundleId::new(legacy_bundle).expect("bundle id"),
    )
    .await;
    match refusal {
        Err(ReviewPreparationError::Review(PostgresReviewError::CalibrationRefused(refusal))) => {
            assert_eq!(
                refusal,
                CalibrationRefusal::IncompatibleIdentityScheme {
                    stored: 1,
                    current: 2,
                }
            );
        }
        Err(other) => panic!("expected an incompatible identity, got: {other}"),
        Ok(_) => panic!("a legacy bundle admitted a run"),
    }

    // The legacy bundle still cannot admit consequential automation. Advisory
    // screening is independently available without calibration under V1.
    sweep_second_reviews(&legacy).await.expect("sweep");
    let runs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM review_run_manifests WHERE project_id=$1")
            .bind(project_id.as_uuid())
            .fetch_one(&legacy)
            .await
            .expect("run count");
    assert_eq!(
        runs, 1,
        "the sweep admits advisory work independently of legacy calibration"
    );
    let origin: String =
        sqlx::query_scalar("SELECT origin->>'kind' FROM review_run_manifests WHERE project_id=$1")
            .bind(project_id.as_uuid())
            .fetch_one(&legacy)
            .await
            .expect("advisory origin");
    assert_eq!(origin, "advisory_triggered");
    assert_eq!(
        second_review_status(
            &legacy,
            project_id.as_uuid(),
            AutonomyTask::TitleAbstractScreening
        )
        .await
        .expect("status"),
        SecondReviewStatus::Automatic
    );

    // The legacy shape cannot be written again.
    let legacy_again = sqlx::query(
        "INSERT INTO review_calibration_bundles
         (id,project_id,definition_key,semantic_bundle_hash,evaluation_set_id,
          thresholds,metrics,reviewer_metadata,status,evaluated_at,identity_scheme)
         VALUES ($1,$2,'screening',$3,'legacy-again',$4,$5,$6,'passing',now(),1)",
    )
    .bind(Uuid::new_v4())
    .bind(project_id.as_uuid())
    .bind("b".repeat(64))
    .bind(json!({}))
    .bind(json!({}))
    .bind(json!({}))
    .execute(&legacy)
    .await
    .expect_err("scheme 1 can no longer be inserted");
    assert!(rejection_message(legacy_again).contains("is read-only"));

    legacy.close().await;
}

#[tokio::test]
async fn new_calibration_bundles_must_state_a_stage_and_a_component_snapshot() {
    let Some(url) = database_url() else { return };
    let pool = pool_for(&url).await;
    migrate(&pool).await.expect("migrations apply");
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'calibration schema')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("project inserts");

    // Screening must name its stage.
    let missing_stage = insert_raw_bundle(
        &pool,
        project_id,
        "screening",
        None,
        2,
        Some(snapshot(2, "screening", None)),
    )
    .await
    .expect_err("a screening bundle without a stage is rejected");
    assert!(
        rejection_message(missing_stage).contains("review_calibration_bundles_stage_scope_check")
    );

    // Other definitions must not carry a stage.
    let staged_duplicate = insert_raw_bundle(
        &pool,
        project_id,
        "duplicate_detection",
        Some("title_abstract"),
        2,
        Some(snapshot(2, "duplicate_detection", Some("title_abstract"))),
    )
    .await
    .expect_err("a non-screening bundle with a stage is rejected");
    assert!(
        rejection_message(staged_duplicate)
            .contains("review_calibration_bundles_stage_scope_check")
    );

    // A snapshot must agree with the stage column.
    let disagreeing = insert_raw_bundle(
        &pool,
        project_id,
        "screening",
        Some("title_abstract"),
        2,
        Some(snapshot(2, "screening", Some("full_text"))),
    )
    .await
    .expect_err("a snapshot for another stage is rejected");
    assert!(
        rejection_message(disagreeing).contains("review_calibration_bundles_identity_shape_check")
    );

    // A scheme 2 bundle without a snapshot is rejected: a NULL check result must not pass.
    let no_snapshot = insert_raw_bundle(
        &pool,
        project_id,
        "screening",
        Some("title_abstract"),
        2,
        None,
    )
    .await
    .expect_err("a scheme 2 bundle needs its component snapshot");
    assert!(
        rejection_message(no_snapshot).contains("review_calibration_bundles_identity_shape_check")
    );

    // Scheme 1 is refused by the insert trigger, whatever the other columns say.
    let legacy_scheme = insert_raw_bundle(&pool, project_id, "screening", None, 1, None)
        .await
        .expect_err("scheme 1 cannot be written");
    assert!(
        rejection_message(legacy_scheme)
            .contains("legacy calibration identity scheme 1 is read-only")
    );

    // New rows must state their scheme, because the migration dropped the default.
    let unstated = sqlx::query(
        "INSERT INTO review_calibration_bundles
         (id,project_id,definition_key,stage,identity_snapshot,semantic_bundle_hash,
          evaluation_set_id,thresholds,metrics,reviewer_metadata,status,evaluated_at)
         VALUES ($1,$2,'screening','title_abstract',$3,$4,'raw-test',$5,$6,$7,'passing',now())",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(snapshot(2, "screening", Some("title_abstract")))
    .bind("c".repeat(64))
    .bind(json!({}))
    .bind(json!({}))
    .bind(json!({}))
    .execute(&pool)
    .await
    .expect_err("the scheme must be stated");
    assert!(rejection_message(unstated).contains("identity_scheme"));

    // A correctly shaped stage-scoped bundle is accepted.
    insert_raw_bundle(
        &pool,
        project_id,
        "screening",
        Some("full_text"),
        2,
        Some(snapshot(2, "screening", Some("full_text"))),
    )
    .await
    .expect("a stage-scoped scheme 2 bundle is accepted");

    // The gate records only the refusal kinds the sweep writes, with their reasons.
    let unknown_refusal = sqlx::query(
        "INSERT INTO second_review_gate (project_id, stage, refusal)
         VALUES ($1,'title_abstract','calibration_unknown')",
    )
    .bind(project_id)
    .execute(&pool)
    .await
    .expect_err("an unknown refusal kind is rejected");
    assert!(rejection_message(unknown_refusal).contains("second_review_gate_refusal_check"));
    sqlx::query(
        "INSERT INTO second_review_gate (project_id, stage, refusal, reasons)
         VALUES ($1,'title_abstract','calibration_stage_mismatch',ARRAY['models'])",
    )
    .bind(project_id)
    .execute(&pool)
    .await
    .expect("a stage mismatch refusal is recorded with its reasons");

    let index: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_indexes
         WHERE tablename='review_calibration_bundles'
           AND indexname='review_calibration_bundles_stage_admission_idx'",
    )
    .fetch_one(&pool)
    .await
    .expect("index lookup");
    assert_eq!(index, 1, "stage-scoped lookups have their index");

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("fixtures clean up");
}
