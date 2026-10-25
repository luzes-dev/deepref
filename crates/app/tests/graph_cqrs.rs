#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]

use deepref_app::{
    enqueue_metrics_recompute, get_projection_status, load_project_graph_strict,
    rebuild_project_projection, recompute_project_metrics,
};
use deepref_domain::graph::{GraphFieldSelection, ProjectionState};
use sqlx::PgPool;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
}

#[tokio::test]
async fn brand_new_project_never_marks_revision_zero_fresh_absent_projection() {
    let Some(pool) = database().await else {
        return;
    };

    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name) VALUES ($1, 'New Project')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("project created");

    // 1. Status before any projection has run
    let status = get_projection_status(&pool, project_id)
        .await
        .expect("get status")
        .expect("project exists");

    assert_eq!(status.project_id, project_id);
    // MUST be Pending, NEVER Fresh/Ready absent actual successful projection!
    assert_eq!(status.state, ProjectionState::Pending);
    assert_eq!(status.requested_revision, 0);
    assert_eq!(status.applied_revision, 0);
    assert_eq!(status.watermark, 0);
    assert_eq!(status.lag, 0);
    assert!(status.last_success_at.is_none());
    assert!(status.last_error.is_none());
    assert!(status.rebuild_state.is_none());

    // 2. Nonexistent project returns None (no global NULL row leakage)
    let nonexistent_id = Uuid::new_v4();
    let nonexistent_status = get_projection_status(&pool, nonexistent_id)
        .await
        .expect("get status");
    assert!(nonexistent_status.is_none());

    // Clean up
    let _ = sqlx::query("DELETE FROM projects WHERE id = $1")
        .bind(project_id)
        .execute(&pool)
        .await;
}

#[tokio::test]
async fn empty_project_after_successful_recompute_is_ready() {
    let Some(pool) = database().await else {
        return;
    };

    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name) VALUES ($1, 'Empty Project')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("project created");

    // Run successful recompute on project
    recompute_project_metrics(&pool, project_id)
        .await
        .expect("recompute succeeds");

    let status = get_projection_status(&pool, project_id)
        .await
        .expect("get status")
        .expect("project exists");

    // Now it is truthfully Ready/Fresh with watermark 0 and last_success_at set
    assert_eq!(status.state, ProjectionState::Ready);
    assert_eq!(status.requested_revision, 0);
    assert_eq!(status.applied_revision, 0);
    assert_eq!(status.watermark, 0);
    assert_eq!(status.lag, 0);
    assert!(status.last_success_at.is_some());
    assert!(status.last_error.is_none());

    // Clean up
    let _ = sqlx::query("DELETE FROM projection_state WHERE project_id = $1")
        .bind(project_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM projects WHERE id = $1")
        .bind(project_id)
        .execute(&pool)
        .await;
}

#[tokio::test]
async fn canonical_mutation_invalidation_and_strict_read() {
    let Some(pool) = database().await else {
        return;
    };

    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name) VALUES ($1, 'Mutation Project')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("project created");

    // Initialize with recompute
    recompute_project_metrics(&pool, project_id)
        .await
        .expect("recompute succeeds");

    let before = get_projection_status(&pool, project_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.state, ProjectionState::Ready);

    // Simulate canonical mutation: enqueue recompute inside transaction
    let mut tx = pool.begin().await.unwrap();
    let job_id = enqueue_metrics_recompute(&mut tx, project_id)
        .await
        .expect("enqueue recompute");
    assert_ne!(job_id, Uuid::nil());
    tx.commit().await.unwrap();

    // Live projection status must immediately report Lagging with lag = 1
    let lagging_status = get_projection_status(&pool, project_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lagging_status.state, ProjectionState::Lagging);
    assert_eq!(lagging_status.requested_revision, 1);
    assert_eq!(lagging_status.applied_revision, 0);
    assert_eq!(lagging_status.lag, 1);

    // Strict read returns bounded graph with explicit lagging status (truthful, non-blocking)
    let (graph, strict_status) =
        load_project_graph_strict(&pool, project_id, GraphFieldSelection::all())
            .await
            .expect("strict read succeeds");
    assert_eq!(strict_status.state, ProjectionState::Lagging);
    assert_eq!(strict_status.lag, 1);
    assert_eq!(graph.nodes.len(), 0);

    // When worker executes recompute, state catches up to Ready
    recompute_project_metrics(&pool, project_id)
        .await
        .expect("catchup succeeds");

    let caught_up = get_projection_status(&pool, project_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(caught_up.state, ProjectionState::Ready);
    assert_eq!(caught_up.requested_revision, 1);
    assert_eq!(caught_up.applied_revision, 1);
    assert_eq!(caught_up.watermark, 1);
    assert_eq!(caught_up.lag, 0);

    // Clean up
    let _ = sqlx::query("DELETE FROM jobs WHERE id = $1")
        .bind(job_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM projection_state WHERE project_id = $1")
        .bind(project_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM projects WHERE id = $1")
        .bind(project_id)
        .execute(&pool)
        .await;
}

#[tokio::test]
async fn idempotency_and_corruption_rebuild() {
    let Some(pool) = database().await else {
        return;
    };

    let project_id = Uuid::new_v4();
    let report_a = Uuid::new_v4();
    let report_b = Uuid::new_v4();

    sqlx::query("INSERT INTO projects (id, name) VALUES ($1, 'Rebuild Project')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("project created");

    for (id, title) in [(report_a, "Report A"), (report_b, "Report B")] {
        sqlx::query(
            "INSERT INTO reports (id, title, publication_year, total_citations, references_count, raw)
             VALUES ($1, $2, 2024, 10, 1, '{}'::jsonb)",
        )
        .bind(id)
        .bind(title)
        .execute(&pool)
        .await
        .unwrap();
    }

    sqlx::query("INSERT INTO project_reports (project_id, report_id) VALUES ($1, $2), ($1, $3)")
        .bind(project_id)
        .bind(report_a)
        .bind(report_b)
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query("INSERT INTO citations (project_id, source_report_id, target_report_id) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(report_a)
        .bind(report_b)
        .execute(&pool)
        .await
        .unwrap();

    // 1. Initial recompute
    recompute_project_metrics(&pool, project_id)
        .await
        .expect("recompute succeeds");

    let status = get_projection_status(&pool, project_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.state, ProjectionState::Ready);

    // Verify snapshot was created
    let snapshot_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM metric_snapshots WHERE project_id = $1")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(snapshot_count > 0);

    // Re-running recompute is idempotent
    recompute_project_metrics(&pool, project_id)
        .await
        .expect("re-recompute succeeds idempotently");

    // 2. Corrupt read model
    sqlx::query("UPDATE project_reports SET rank_score = 999.0 WHERE project_id = $1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM metric_snapshots WHERE project_id = $1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();

    // 3. Trigger rebuild
    let rebuild_job_id = rebuild_project_projection(&pool, project_id)
        .await
        .expect("rebuild enqueued");

    // Immediate status is Rebuilding
    let rebuilding_status = get_projection_status(&pool, project_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rebuilding_status.state, ProjectionState::Rebuilding);
    assert_eq!(
        rebuilding_status.rebuild_state.as_deref(),
        Some("rebuilding")
    );

    // metrics_computed_at was cleared
    let uncomputed_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM project_reports WHERE project_id = $1 AND metrics_computed_at IS NULL",
    )
    .bind(project_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(uncomputed_count, 2);

    // 4. Worker executes rebuild job
    recompute_project_metrics(&pool, project_id)
        .await
        .expect("rebuild job processing succeeds");

    // Post-rebuild status is Ready, rebuild_state is cleared
    let recovered_status = get_projection_status(&pool, project_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered_status.state, ProjectionState::Ready);
    assert!(recovered_status.rebuild_state.is_none());

    // Corrupted score 999.0 is repaired
    let max_score: f64 =
        sqlx::query_scalar("SELECT max(rank_score) FROM project_reports WHERE project_id = $1")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(max_score < 10.0, "rank score must be repaired from 999.0");

    // Snapshot is restored
    let snapshot_restored: i64 =
        sqlx::query_scalar("SELECT count(*) FROM metric_snapshots WHERE project_id = $1")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(snapshot_restored > 0);

    // Clean up
    let _ = sqlx::query("DELETE FROM jobs WHERE id = $1")
        .bind(rebuild_job_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM metric_snapshots WHERE project_id = $1")
        .bind(project_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM citations WHERE project_id = $1")
        .bind(project_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM project_reports WHERE project_id = $1")
        .bind(project_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM reports WHERE id IN ($1, $2)")
        .bind(report_a)
        .bind(report_b)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM projection_state WHERE project_id = $1")
        .bind(project_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM projects WHERE id = $1")
        .bind(project_id)
        .execute(&pool)
        .await;
}
