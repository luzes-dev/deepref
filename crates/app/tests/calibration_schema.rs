#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Current calibration rows require a stage-scoped component identity.

use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

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
async fn new_calibration_bundles_must_state_a_stage_and_a_component_snapshot() {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 4).await
    else {
        return;
    };
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
