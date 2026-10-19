#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! The staged existing-project migration (03a): rolling-safe, idempotent, and
//! it preserves every project ID and scientific row.
//!
//! Each test replays the migrations as the rollout does: the previous
//! application release's schema first, the legacy rows an old binary wrote,
//! then the workspace migrations on top.

use deepref_app::{MIGRATOR, workspaces::MIGRATION_WORKSPACE_ID};
use sqlx::{PgPool, migrate::Migrator, postgres::PgPoolOptions};
use uuid::Uuid;

/// The first workspace-tenancy migration. Migrations before it are the schema
/// of the previous application release.
const WORKSPACE_TENANCY: i64 = 87;

async fn pool_for(url: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(4)
        .connect(url)
        .await
        .expect("database is reachable")
}

/// The migrations the previous release ran.
fn through_previous_release() -> Migrator {
    Migrator::with_migrations(
        MIGRATOR
            .iter()
            .filter(|migration| migration.version < WORKSPACE_TENANCY)
            .cloned()
            .collect(),
    )
}

/// Writes what a pre-workspaces deployment held: users, projects (there is no
/// `workspace_id` column yet) and scientific rows, all with fixed IDs the test
/// can recognize afterwards.
async fn write_legacy_rows(pool: &PgPool) -> Vec<Uuid> {
    let user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, workos_user_id, primary_email, display_name, status)
         VALUES ($1, 'legacy_user', 'legacy@example.org', 'Legacy User', 'active')",
    )
    .bind(user_id)
    .execute(pool)
    .await
    .expect("legacy user inserts");

    let mut project_ids = Vec::new();
    for name in ["Legacy project one", "Legacy project two"] {
        let project_id = Uuid::new_v4();
        sqlx::query("INSERT INTO projects (id, name, description) VALUES ($1, $2, 'legacy')")
            .bind(project_id)
            .bind(name)
            .execute(pool)
            .await
            .expect("legacy project inserts");
        project_ids.push(project_id);
    }

    for project_id in &project_ids {
        let report_id = Uuid::new_v4();
        sqlx::query("INSERT INTO reports (id, title, abstract_text) VALUES ($1, 'Legacy report', 'Scientific record')")
            .bind(report_id)
            .execute(pool)
            .await
            .expect("legacy report inserts");
        sqlx::query("INSERT INTO project_reports (project_id, report_id) VALUES ($1, $2)")
            .bind(project_id)
            .bind(report_id)
            .execute(pool)
            .await
            .expect("legacy project report inserts");
    }
    project_ids
}

async fn row_texts(pool: &PgPool, sql: &str) -> Vec<String> {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .fetch_all(pool)
        .await
        .expect("rows load")
}

async fn project_workspaces(pool: &PgPool) -> Vec<(Uuid, Option<Uuid>)> {
    sqlx::query_as("SELECT id, workspace_id FROM projects ORDER BY id")
        .fetch_all(pool)
        .await
        .expect("projects load")
}

#[tokio::test]
async fn every_project_gets_the_migration_workspace_and_no_row_changes_shape() {
    let Some(url) = deepref_app::test_isolation::isolated_empty_url_from_env("DATABASE_URL").await
    else {
        return;
    };
    let pool = pool_for(&url).await;
    through_previous_release()
        .run(&pool)
        .await
        .expect("the previous release's migrations apply");

    let project_ids = write_legacy_rows(&pool).await;
    let projects_before = row_texts(&pool, "SELECT row_to_json(p)::text FROM projects p").await;
    let reports_before = row_texts(&pool, "SELECT row_to_json(r)::text FROM reports r").await;
    let links_before = row_texts(
        &pool,
        "SELECT row_to_json(pr)::text FROM project_reports pr",
    )
    .await;
    assert_eq!(projects_before.len(), 2);
    assert_eq!(reports_before.len(), 2);

    // The workspace migrations on top of that data.
    deepref_app::migrate(&pool)
        .await
        .expect("workspace migrations apply");

    for (project_id, workspace_id) in project_workspaces(&pool).await {
        assert!(
            project_ids.contains(&project_id),
            "project IDs are preserved"
        );
        assert_eq!(
            workspace_id,
            Some(MIGRATION_WORKSPACE_ID.as_uuid()),
            "every project belongs to the migration workspace"
        );
    }
    let workspaces: i64 = sqlx::query_scalar("SELECT count(*) FROM workspaces")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(workspaces, 1, "exactly one migration workspace is created");

    // Scientific rows are byte-identical: same rows, same column values.
    assert_eq!(
        row_texts(&pool, "SELECT row_to_json(r)::text FROM reports r").await,
        reports_before
    );
    assert_eq!(
        row_texts(
            &pool,
            "SELECT row_to_json(pr)::text FROM project_reports pr"
        )
        .await,
        links_before
    );
    // Projects keep every value they had; only the new column is set.
    let projects_after = row_texts(&pool, "SELECT row_to_json(p)::text FROM projects p").await;
    assert_eq!(projects_after.len(), projects_before.len());
    for (before, after) in projects_before.iter().zip(&projects_after) {
        let before: serde_json::Value = serde_json::from_str(before).unwrap();
        let after: serde_json::Value = serde_json::from_str(after).unwrap();
        for (key, value) in before.as_object().unwrap() {
            assert_eq!(&after[key], value, "column {key} is unchanged");
        }
    }

    // The pre-existing user keeps access to their projects.
    let memberships: Vec<(String, String)> = sqlx::query_as(
        "SELECT role, grant_source FROM workspace_memberships
         WHERE workspace_id = $1",
    )
    .bind(MIGRATION_WORKSPACE_ID.as_uuid())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        memberships,
        vec![("owner".to_owned(), "local".to_owned())],
        "pre-existing users own the migration workspace as a local grant"
    );

    // Idempotent: rerunning the backfill moves nothing and changes nothing.
    for _ in 0..2 {
        let moved: i32 = sqlx::query_scalar("SELECT backfill_project_workspaces_batch(500)")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(moved, 0, "a rerun of the backfill moves no rows");
    }
    assert_eq!(
        row_texts(&pool, "SELECT row_to_json(p)::text FROM projects p").await,
        projects_after
    );
    let memberships_after: i64 = sqlx::query_scalar("SELECT count(*) FROM workspace_memberships")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(memberships_after, 1, "no duplicate membership is created");
}

#[tokio::test]
async fn an_old_binary_insert_without_workspace_id_survives_the_rollout() {
    let Some(url) = deepref_app::test_isolation::isolated_empty_url_from_env("DATABASE_URL").await
    else {
        return;
    };
    let pool = pool_for(&url).await;
    through_previous_release()
        .run(&pool)
        .await
        .expect("the previous release's migrations apply");

    // A project from before the column existed.
    let before_column = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name) VALUES ($1, 'pre-migration project')")
        .bind(before_column)
        .execute(&pool)
        .await
        .unwrap();

    // Only the expand migration (0087) is deployed: the previous release is
    // still serving and inserts projects the old way.
    let through_expand = Migrator::with_migrations(
        MIGRATOR
            .iter()
            .filter(|migration| migration.version < WORKSPACE_TENANCY + 1)
            .cloned()
            .collect(),
    );
    through_expand
        .run(&pool)
        .await
        .expect("the expand migration applies");

    let during_rollout = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name) VALUES ($1, 'old binary project')")
        .bind(during_rollout)
        .execute(&pool)
        .await
        .expect("an old-binary insert without workspace_id still works");

    // The rest of the migration: backfill, validation, constraint.
    deepref_app::migrate(&pool).await.expect("backfill applies");

    let workspaces: Vec<Option<Uuid>> =
        sqlx::query_scalar("SELECT workspace_id FROM projects WHERE id = ANY($1) ORDER BY id")
            .bind([before_column, during_rollout])
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        workspaces,
        vec![Some(MIGRATION_WORKSPACE_ID.as_uuid()); 2],
        "backfill and trigger assign the migration workspace"
    );

    // Even after the constraint exists, an old binary's insert is accepted and
    // assigned, so the rollout stays rolling-safe.
    let after_constraint = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name) VALUES ($1, 'still the old shape')")
        .bind(after_constraint)
        .execute(&pool)
        .await
        .expect("the old insert shape still works beside the constraint");
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

    // The lookup index exists, built by its own no-transaction migration.
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
async fn users_at_migration_time_are_marked_onboarded_and_the_rest_are_left_for_sign_in() {
    let Some(url) = deepref_app::test_isolation::isolated_empty_url_from_env("DATABASE_URL").await
    else {
        return;
    };
    let pool = pool_for(&url).await;
    through_previous_release().run(&pool).await.unwrap();
    write_legacy_rows(&pool).await;
    deepref_app::migrate(&pool).await.unwrap();
    let onboarded: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE onboarded_at IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        onboarded, 1,
        "the existing user holds the migration membership"
    );
    let unverified: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE email_verified")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        unverified, 0,
        "no address is trusted until the provider says so"
    );
}
