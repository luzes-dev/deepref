#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Current workspace defaults and bounded repair commands.

use deepref_app::workspaces::MIGRATION_WORKSPACE_ID;
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

async fn pool_for(url: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(4)
        .connect(url)
        .await
        .expect("database is reachable")
}

#[tokio::test]
async fn current_project_inserts_assign_missing_workspace_and_preserve_explicit_scope() {
    let Some(url) = deepref_app::test_isolation::isolated_empty_url_from_env("DATABASE_URL").await
    else {
        return;
    };
    let pool = pool_for(&url).await;
    deepref_app::migrate(&pool).await.expect("baseline applies");

    // Missing workspace IDs are assigned by the current schema default trigger.
    let after_constraint = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name) VALUES ($1, 'implicit workspace')")
        .bind(after_constraint)
        .execute(&pool)
        .await
        .expect("an implicit workspace is assigned");
    let assigned: Option<Uuid> =
        sqlx::query_scalar("SELECT workspace_id FROM projects WHERE id = $1")
            .bind(after_constraint)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(assigned, Some(MIGRATION_WORKSPACE_ID.as_uuid()));

    // A new-style insert names its workspace explicitly and is left alone.
    let explicit = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workspaces (id, kind, name, slug)
         VALUES ($1, 'personal', 'Someone', 'someone-workspace')",
    )
    .bind(explicit)
    .execute(&pool)
    .await
    .unwrap();
    let explicit_project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, workspace_id) VALUES ($1, 'new project', $2)")
        .bind(explicit_project)
        .bind(explicit)
        .execute(&pool)
        .await
        .unwrap();
    let assigned: Option<Uuid> =
        sqlx::query_scalar("SELECT workspace_id FROM projects WHERE id = $1")
            .bind(explicit_project)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(assigned, Some(explicit));
}

#[tokio::test]
async fn the_constraint_refuses_a_project_that_resolves_to_no_workspace() {
    let Some(url) = deepref_app::test_isolation::isolated_empty_url_from_env("DATABASE_URL").await
    else {
        return;
    };
    let pool = pool_for(&url).await;
    deepref_app::migrate(&pool).await.expect("migrations apply");

    // The trigger fills the migration workspace, so a NULL can only appear if
    // the trigger is gone; the foreign key then refuses the dangling
    // reference.
    sqlx::query("ALTER TABLE projects DISABLE TRIGGER projects_workspace_default_trigger")
        .execute(&pool)
        .await
        .unwrap();
    let rejected =
        sqlx::query("INSERT INTO projects (id, name, workspace_id) VALUES ($1, 'dangling', $2)")
            .bind(Uuid::new_v4())
            .bind(Uuid::new_v4())
            .execute(&pool)
            .await;
    assert!(rejected.is_err(), "a project must reference a workspace");

    // And the validation of a rerun catches any unassigned row.
    sqlx::query("ALTER TABLE projects DISABLE TRIGGER projects_workspace_default_trigger")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO projects (id, name) VALUES ($1, 'unassigned')")
        .bind(Uuid::new_v4())
        .execute(&pool)
        .await
        .unwrap();
    let backfilled: i32 = sqlx::query_scalar("SELECT backfill_project_workspaces_batch(500)")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(backfilled, 1, "the backfill picks up an unassigned row");
    let unassigned: i64 =
        sqlx::query_scalar("SELECT count(*) FROM projects WHERE workspace_id IS NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(unassigned, 0);
}

#[tokio::test]
async fn the_backfill_commits_each_batch_and_validation_reports_without_changing_anything() {
    let Some(url) = deepref_app::test_isolation::isolated_empty_url_from_env("DATABASE_URL").await
    else {
        return;
    };
    let pool = pool_for(&url).await;
    deepref_app::migrate(&pool).await.expect("migrations apply");

    // The current schema includes the workspace lookup index.
    let indexed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_indexes WHERE indexname = 'projects_workspace_idx'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(indexed, 1);

    // Rows the trigger would have assigned, written without it as a crashed
    // earlier run would have left them.
    sqlx::query("ALTER TABLE projects DISABLE TRIGGER projects_workspace_default_trigger")
        .execute(&pool)
        .await
        .unwrap();
    for index in 0..5 {
        sqlx::query("INSERT INTO projects (id, name) VALUES ($1, $2)")
            .bind(Uuid::new_v4())
            .bind(format!("unassigned {index}"))
            .execute(&pool)
            .await
            .unwrap();
    }

    // Validation refuses while rows are unassigned and modifies nothing.
    let refused = deepref_app::validate_project_workspaces(&pool).await;
    assert!(
        matches!(
            refused,
            Err(deepref_app::WorkspaceError::ProjectWorkspaceValidation {
                total: 5,
                unassigned: 5,
                orphaned: 0
            })
        ),
        "{refused:?}"
    );
    let still_unassigned: i64 =
        sqlx::query_scalar("SELECT count(*) FROM projects WHERE workspace_id IS NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(still_unassigned, 5, "validation changes nothing");

    // Each batch commits on its own: 5 rows in batches of 2 is 3 batches.
    let report = deepref_app::backfill_project_workspaces(&pool, 2)
        .await
        .expect("backfill completes and validates");
    assert_eq!(
        (report.batches, report.moved, report.total_projects),
        (3, 5, 5)
    );
    let rerun = deepref_app::backfill_project_workspaces(&pool, 2)
        .await
        .unwrap();
    assert_eq!(
        (rerun.batches, rerun.moved),
        (0, 0),
        "a rerun moves nothing"
    );
}

#[tokio::test]
async fn existing_members_are_marked_onboarded_by_the_batched_command() {
    let Some(url) = deepref_app::test_isolation::isolated_empty_url_from_env("DATABASE_URL").await
    else {
        return;
    };
    let pool = pool_for(&url).await;
    deepref_app::migrate(&pool).await.unwrap();
    let user = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id,workos_user_id,primary_email,status) VALUES ($1,$2,'existing@example.test','active')")
        .bind(user).bind(format!("existing-{user}")).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO workspace_memberships (id,workspace_id,user_id,role,status,grant_source) VALUES ($1,$2,$3,'owner','active','local')")
        .bind(Uuid::new_v4()).bind(MIGRATION_WORKSPACE_ID.as_uuid()).bind(user).execute(&pool).await.unwrap();
    let marked = |pool: PgPool| async move {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users WHERE onboarded_at IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    assert_eq!(
        marked(pool.clone()).await,
        0,
        "an existing member stays unmarked until the command runs"
    );
    let verified: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE verified_email IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        verified, 0,
        "no address is trusted until the provider says so"
    );

    let report = deepref_app::backfill_project_workspaces(&pool, 1)
        .await
        .unwrap();
    assert_eq!(report.users_marked_onboarded, 1);
    assert_eq!(marked(pool.clone()).await, 1);
    let rerun = deepref_app::backfill_project_workspaces(&pool, 1)
        .await
        .unwrap();
    assert_eq!(rerun.users_marked_onboarded, 0);
}

#[tokio::test]
async fn validation_never_fails_falsely_beside_concurrent_trigger_inserts() {
    let Some(url) = deepref_app::test_isolation::isolated_empty_url_from_env("DATABASE_URL").await
    else {
        return;
    };
    let pool = pool_for(&url).await;
    deepref_app::migrate(&pool).await.expect("migrations apply");

    // An old-shape writer inserts without a workspace; the trigger assigns it.
    let writer_pool = pool.clone();
    let writer = tokio::spawn(async move {
        for index in 0..300 {
            sqlx::query("INSERT INTO projects (id, name) VALUES ($1, $2)")
                .bind(Uuid::new_v4())
                .bind(format!("concurrent {index}"))
                .execute(&writer_pool)
                .await
                .expect("old-shape insert");
        }
    });
    for _ in 0..150 {
        deepref_app::validate_project_workspaces(&pool)
            .await
            .expect("a concurrent insert never makes validation disagree with itself");
    }
    writer.await.unwrap();
    assert_eq!(
        deepref_app::validate_project_workspaces(&pool)
            .await
            .unwrap(),
        300
    );
}
