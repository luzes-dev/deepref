#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! "Delete my account" (handoff 11, BUSINESS_DECISIONS): request, cancel and
//! the 30-day completion, which anonymizes the account while keeping the user
//! id, the scientific provenance and the whole audit history.

use deepref_app::account_deletion::{
    DeletionCancellationOutcome, DeletionRequestOutcome, FORMER_MEMBER_LABEL,
    cancel_account_deletion, complete_account_deletions, request_account_deletion,
};
use deepref_app::identity::AuditRequest;
use deepref_app::workspaces::MIGRATION_WORKSPACE_ID;
use deepref_domain::{UserId, WorkspaceId};
use sqlx::{PgPool, Row};
use uuid::Uuid;

const AUDIT: AuditRequest<'_> = AuditRequest {
    request_id: Some("0b0e0d0c-0000-4000-8000-000000000011"),
    session_ref: None,
};

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
}

async fn make_user(pool: &PgPool, id: Uuid, email: &str, name: &str) -> UserId {
    sqlx::query(
        "INSERT INTO users (id, workos_user_id, primary_email, verified_email, display_name, status)
         VALUES ($1, $2, $3, $3, $4, 'active')",
    )
    .bind(id)
    .bind(format!("user_{id}"))
    .bind(email)
    .bind(name)
    .execute(pool)
    .await
    .unwrap();
    UserId::new(id)
}

async fn make_workspace(pool: &PgPool, id: Uuid, slug: &str) -> WorkspaceId {
    sqlx::query(
        "INSERT INTO workspaces (id, kind, name, slug) VALUES ($1, 'organization', $2, $3)",
    )
    .bind(id)
    .bind(slug)
    .bind(slug)
    .execute(pool)
    .await
    .unwrap();
    WorkspaceId::new(id)
}

async fn add_member(pool: &PgPool, workspace: WorkspaceId, user: UserId, role: &str) {
    sqlx::query(
        "INSERT INTO workspace_memberships (id, workspace_id, user_id, role, status, grant_source)
         VALUES ($1, $2, $3, $4, 'active', 'local')",
    )
    .bind(Uuid::new_v4())
    .bind(workspace.as_uuid())
    .bind(user.as_uuid())
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
}

async fn make_project(pool: &PgPool, id: Uuid, workspace: WorkspaceId, name: &str) {
    sqlx::query("INSERT INTO projects (id, name, workspace_id) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(name)
        .bind(workspace.as_uuid())
        .execute(pool)
        .await
        .unwrap();
}

async fn events(pool: &PgPool, event: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM admin_audit_events WHERE event_type = $1")
        .bind(event)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn age_request(pool: &PgPool, user: UserId, days: i64) {
    sqlx::query(
        "UPDATE users SET deletion_requested_at = now() - make_interval(days => $2) WHERE id = $1",
    )
    .bind(user.as_uuid())
    .bind(days as i32)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn a_deletion_request_and_its_cancellation_are_each_audited_once() {
    let Some(pool) = database().await else {
        return;
    };
    let user = make_user(&pool, Uuid::new_v4(), "leaving@example.org", "Leaving").await;

    assert_eq!(
        request_account_deletion(&pool, user, AUDIT).await.unwrap(),
        DeletionRequestOutcome::Requested
    );
    assert_eq!(
        request_account_deletion(&pool, user, AUDIT).await.unwrap(),
        DeletionRequestOutcome::AlreadyPending
    );
    assert_eq!(events(&pool, "account.deletion_requested").await, 1);

    assert_eq!(
        cancel_account_deletion(&pool, user, AUDIT).await.unwrap(),
        DeletionCancellationOutcome::Cancelled
    );
    assert_eq!(
        cancel_account_deletion(&pool, user, AUDIT).await.unwrap(),
        DeletionCancellationOutcome::NoRequest
    );
    assert_eq!(events(&pool, "account.deletion_cancelled").await, 1);

    // The row names itself: principal and target are the same user.
    let row = sqlx::query(
        "SELECT principal_kind, principal_user_id, target_user_id, request_id
         FROM admin_audit_events WHERE event_type = 'account.deletion_requested'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("principal_kind"), "user");
    assert_eq!(row.get::<Uuid, _>("principal_user_id"), user.as_uuid());
    assert_eq!(row.get::<Uuid, _>("target_user_id"), user.as_uuid());
    assert_eq!(
        row.get::<Option<String>, _>("request_id").as_deref(),
        Some(AUDIT.request_id.unwrap())
    );

    // A cancelled account can request again.
    assert_eq!(
        request_account_deletion(&pool, user, AUDIT).await.unwrap(),
        DeletionRequestOutcome::Requested
    );
    assert_eq!(events(&pool, "account.deletion_requested").await, 2);
}

#[tokio::test]
async fn completion_anonymizes_the_account_and_keeps_history() {
    let Some(pool) = database().await else {
        return;
    };
    let user = make_user(&pool, Uuid::new_v4(), "gone@example.org", "Gone Person").await;
    let colleague = make_user(&pool, Uuid::new_v4(), "kept@example.org", "Kept Person").await;

    // A workspace only the user owns, with content.
    let solo = make_workspace(&pool, Uuid::new_v4(), "solo-workspace").await;
    add_member(&pool, solo, user, "owner").await;
    let solo_project = Uuid::new_v4();
    make_project(&pool, solo_project, solo, "solo project").await;

    // A shared workspace whose contribution stays attributed to the user.
    let shared = make_workspace(&pool, Uuid::new_v4(), "shared-workspace").await;
    add_member(&pool, shared, user, "owner").await;
    add_member(&pool, shared, colleague, "member").await;
    let shared_project = Uuid::new_v4();
    make_project(&pool, shared_project, shared, "shared project").await;
    sqlx::query(
        "INSERT INTO scientific_assignments (project_id, user_id, role, assigned_by)
         VALUES ($1, $2, 'primary_screener', $2)",
    )
    .bind(shared_project)
    .bind(user.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    request_account_deletion(&pool, user, AUDIT).await.unwrap();
    age_request(&pool, user, 31).await;

    let report = complete_account_deletions(&pool, AUDIT).await.unwrap();
    assert_eq!(report.completed, 1);
    assert_eq!(report.re_applied, 0);
    assert_eq!(report.workspaces_deleted, 1);
    assert_eq!(report.projects_deleted, 1);

    // The account is deactivated and anonymized; the user id is stable.
    let row = sqlx::query(
        "SELECT status, primary_email, display_name, verified_email, deleted_at
         FROM users WHERE id = $1",
    )
    .bind(user.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("status"), "deactivated");
    assert_eq!(row.get::<String, _>("primary_email"), FORMER_MEMBER_LABEL);
    assert_eq!(row.get::<String, _>("display_name"), FORMER_MEMBER_LABEL);
    assert!(row.get::<Option<String>, _>("verified_email").is_none());
    assert!(
        row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("deleted_at")
            .is_some()
    );

    // Sessions are revoked.
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM user_sessions WHERE user_id = $1 AND revoked_at IS NULL"
        )
        .bind(user.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );

    // The solely owned workspace and its content are gone through the
    // existing deletions; the shared one and its contribution stay.
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM projects WHERE id = $1")
            .bind(solo_project)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM workspaces WHERE id = $1")
            .bind(solo.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    let shared_row = sqlx::query(
        "SELECT a.user_id, u.display_name
         FROM scientific_assignments a JOIN users u ON u.id = a.user_id
         WHERE a.project_id = $1",
    )
    .bind(shared_project)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(shared_row.get::<Uuid, _>("user_id"), user.as_uuid());
    assert_eq!(
        shared_row.get::<String, _>("display_name"),
        FORMER_MEMBER_LABEL
    );

    // Scientific provenance and the audit history survive by user id.
    assert!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM admin_audit_events
             WHERE principal_user_id = $1 OR target_user_id = $1"
        )
        .bind(user.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap()
            > 0
    );

    // The audit rows of the completion, and the deletion-proof record.
    assert_eq!(events(&pool, "account.deletion_completed").await, 1);
    assert_eq!(events(&pool, "project.deleted").await, 1);
    assert_eq!(events(&pool, "workspace.deleted").await, 1);
    let completed = sqlx::query(
        "SELECT principal_kind, target_user_id FROM admin_audit_events
         WHERE event_type = 'account.deletion_completed'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(completed.get::<String, _>("principal_kind"), "system");
    assert_eq!(completed.get::<Uuid, _>("target_user_id"), user.as_uuid());
    let deactivated = sqlx::query_scalar::<_, String>(
        "SELECT metadata->>'source' FROM admin_audit_events
         WHERE event_type = 'user.deactivated' AND target_user_id = $1",
    )
    .bind(user.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(deactivated, "account_deletion");

    // The fulfillment log holds no content.
    let logged = sqlx::query("SELECT * FROM fulfilled_account_deletions WHERE user_id = $1")
        .bind(user.as_uuid())
        .fetch_optional(&pool)
        .await
        .unwrap();
    let logged = logged.expect("fulfillment logged");
    assert_eq!(logged.columns().len(), 2);
}

#[tokio::test]
async fn completion_is_idempotent_and_reapplies_what_a_restore_undid() {
    let Some(pool) = database().await else {
        return;
    };
    let user = make_user(&pool, Uuid::new_v4(), "back@example.org", "Back Person").await;
    request_account_deletion(&pool, user, AUDIT).await.unwrap();
    age_request(&pool, user, 31).await;

    let first = complete_account_deletions(&pool, AUDIT).await.unwrap();
    assert_eq!(first.completed, 1);
    // Nothing is due any more.
    let second = complete_account_deletions(&pool, AUDIT).await.unwrap();
    assert_eq!(second.completed + second.re_applied, 0);
    assert_eq!(events(&pool, "account.deletion_completed").await, 1);

    // A restore brings the personal data back (docs/security.md,
    // "Restore step"); the log says this deletion was fulfilled once.
    sqlx::query(
        "UPDATE users
         SET status = 'active', primary_email = 'restored@example.org',
             verified_email = 'restored@example.org', display_name = 'Restored Person',
             deleted_at = NULL
         WHERE id = $1",
    )
    .bind(user.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let restored_workspace = make_workspace(&pool, Uuid::new_v4(), "restored-workspace").await;
    add_member(&pool, restored_workspace, user, "owner").await;

    let third = complete_account_deletions(&pool, AUDIT).await.unwrap();
    assert_eq!(third.completed, 0);
    assert_eq!(third.re_applied, 1);
    assert_eq!(third.workspaces_deleted, 1);
    let row = sqlx::query("SELECT status, primary_email, verified_email FROM users WHERE id = $1")
        .bind(user.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("status"), "deactivated");
    assert_eq!(row.get::<String, _>("primary_email"), FORMER_MEMBER_LABEL);
    assert!(row.get::<Option<String>, _>("verified_email").is_none());
    // Re-applying is not a second deletion: the completion record stands once.
    assert_eq!(events(&pool, "account.deletion_completed").await, 1);
}

#[tokio::test]
async fn completion_waits_the_full_grace_period() {
    let Some(pool) = database().await else {
        return;
    };
    let user = make_user(&pool, Uuid::new_v4(), "waiting@example.org", "Waiting").await;
    request_account_deletion(&pool, user, AUDIT).await.unwrap();
    age_request(&pool, user, 29).await;

    let report = complete_account_deletions(&pool, AUDIT).await.unwrap();
    assert_eq!(report, Default::default());
    let row = sqlx::query("SELECT status, primary_email FROM users WHERE id = $1")
        .bind(user.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("status"), "active");
    assert_eq!(row.get::<String, _>("primary_email"), "waiting@example.org");
    assert_eq!(events(&pool, "account.deletion_completed").await, 0);
}

#[tokio::test]
async fn the_migration_workspace_survives_an_account_deletion() {
    let Some(pool) = database().await else {
        return;
    };
    let user = make_user(&pool, Uuid::new_v4(), "only@example.org", "Only Member").await;
    // The user is the migration workspace's only member; it is still kept.
    add_member(&pool, MIGRATION_WORKSPACE_ID, user, "owner").await;
    request_account_deletion(&pool, user, AUDIT).await.unwrap();
    age_request(&pool, user, 31).await;

    let report = complete_account_deletions(&pool, AUDIT).await.unwrap();
    assert_eq!(report.completed, 1);
    assert_eq!(report.workspaces_deleted, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM workspaces WHERE id = $1")
            .bind(MIGRATION_WORKSPACE_ID.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}
