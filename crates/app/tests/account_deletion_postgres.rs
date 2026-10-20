#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! "Delete my account" (handoff 11, BUSINESS_DECISIONS): request, cancel and
//! the 30-day completion, which erases the personal data while keeping the
//! user id, the scientific provenance and the whole audit history.
//!
//! The race tests drive the two seams the completion is built from
//! (`workspace_candidates` and `complete_workspace`) so the interleavings are
//! deterministic: a member joining between the candidate scan and the
//! per-workspace deletion, and a cancellation racing a completion, are
//! reproduced exactly instead of hoped for.

use deepref_app::account_deletion::{
    AccountDeletionError, DeletionCancellationOutcome, DeletionRequestOutcome, FORMER_MEMBER_LABEL,
    PII_METADATA_KEYS, REDACTED, WorkspaceOutcome, cancel_account_deletion,
    complete_account_deletions, complete_workspace, request_account_deletion, workos_tombstone,
    workspace_candidates,
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
        "UPDATE account_deletion_requests
         SET requested_at = now() - make_interval(days => $2)
         WHERE user_id = $1 AND state = 'requested'",
    )
    .bind(user.as_uuid())
    .bind(days as i32)
    .execute(pool)
    .await
    .unwrap();
}

async fn request_state(pool: &PgPool, user: UserId) -> Option<String> {
    sqlx::query_scalar(
        "SELECT state FROM account_deletion_requests
         WHERE user_id = $1 ORDER BY requested_at DESC, id DESC LIMIT 1",
    )
    .bind(user.as_uuid())
    .fetch_optional(pool)
    .await
    .unwrap()
}

/// One session row carrying a sealed provider payload, as sign-in stores it.
async fn make_session(pool: &PgPool, user: UserId, sealed: &str) {
    let mut ref_bytes = [0u8; 32];
    ref_bytes[..16].copy_from_slice(&Uuid::new_v4().to_bytes_le());
    sqlx::query(
        "INSERT INTO user_sessions
           (id, user_id, session_token_hash, sealed_provider_session, session_ref, expires_at)
         VALUES ($1, $2, $3, $4, $3, now() + interval '1 hour')",
    )
    .bind(Uuid::new_v4())
    .bind(user.as_uuid())
    .bind(&ref_bytes[..])
    .bind(sealed)
    .execute(pool)
    .await
    .unwrap();
}

/// One invitation addressed to `email` in `workspace`, in the state asked for.
async fn make_invitation(pool: &PgPool, workspace: WorkspaceId, email: &str, status: &str) -> Uuid {
    let id = Uuid::new_v4();
    let accepted = status == "accepted";
    sqlx::query(
        "INSERT INTO workspace_invitations
           (id, workspace_id, email, role, invited_by, status, expires_at,
            accepted_by_user_id, accepted_at, revoked_at)
         SELECT $1, $2, $3, 'member', u.id, $4, now() + interval '1 day',
                CASE WHEN $5 THEN u.id ELSE NULL END,
                CASE WHEN $5 THEN now() ELSE NULL END,
                CASE WHEN $4 = 'revoked' THEN now() ELSE NULL END
         FROM users u ORDER BY u.created_at LIMIT 1",
    )
    .bind(id)
    .bind(workspace.as_uuid())
    .bind(email)
    .bind(status)
    .bind(accepted)
    .execute(pool)
    .await
    .unwrap();
    id
}

/// The state of one account after completion, as the tests assert it.
async fn account_row(pool: &PgPool, user: UserId) -> (String, String, Option<String>, bool) {
    let row = sqlx::query(
        "SELECT status, primary_email, display_name, verified_email, deleted_at, workos_user_id
         FROM users WHERE id = $1",
    )
    .bind(user.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        row.get::<String, _>("primary_email"),
        FORMER_MEMBER_LABEL,
        "the email is erased"
    );
    assert_eq!(
        row.get::<String, _>("display_name"),
        FORMER_MEMBER_LABEL,
        "the name is erased"
    );
    (
        row.get("status"),
        row.get("workos_user_id"),
        row.get("verified_email"),
        row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("deleted_at")
            .is_some(),
    )
}

/// Every text-like column in the database that still contains one of `needles`.
/// The whole point of completion is that this returns nothing once the account
/// is done: email, name and WorkOS id are gone from every table.
async fn personal_data_leaks(pool: &PgPool, needles: &[&str]) -> Vec<String> {
    let columns: Vec<(String, String)> = sqlx::query_as(
        "SELECT table_name, column_name FROM information_schema.columns
         WHERE table_schema = 'public'
           AND data_type IN ('text', 'character varying', 'character', 'jsonb', 'json')
         ORDER BY table_name, column_name",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    let mut leaks = Vec::new();
    for (table, column) in columns {
        for needle in needles {
            let hits: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT count(*) FROM \"{table}\" WHERE strpos(\"{column}\"::text, $1) > 0"
            )))
            .bind(needle)
            .fetch_one(pool)
            .await
            .unwrap();
            if hits > 0 {
                leaks.push(format!(
                    "{table}.{column} still holds {needle} ({hits} rows)"
                ));
            }
        }
    }
    leaks
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
        request_state(&pool, user).await.as_deref(),
        Some("requested"),
        "the request is an explicit state"
    );

    assert_eq!(
        cancel_account_deletion(&pool, user, AUDIT).await.unwrap(),
        DeletionCancellationOutcome::Cancelled
    );
    assert_eq!(
        request_state(&pool, user).await.as_deref(),
        Some("cancelled"),
        "a cancellation is terminal for that request"
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

    // A cancelled account can request again: a NEW request row, so the 30-day
    // wait restarts from scratch.
    assert_eq!(
        request_account_deletion(&pool, user, AUDIT).await.unwrap(),
        DeletionRequestOutcome::Requested
    );
    assert_eq!(events(&pool, "account.deletion_requested").await, 2);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM account_deletion_requests WHERE user_id = $1"
        )
        .bind(user.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap(),
        2,
        "a fresh request is a fresh row"
    );
    age_request(&pool, user, 29).await;
    let report = complete_account_deletions(&pool, AUDIT).await.unwrap();
    assert_eq!(
        report.completed, 0,
        "the restarted clock is not yet due at 29 days"
    );
}

#[tokio::test]
async fn completion_erases_the_personal_data_and_keeps_history() {
    let Some(pool) = database().await else {
        return;
    };
    let user = make_user(&pool, Uuid::new_v4(), "gone@example.org", "Gone Person").await;
    let colleague = make_user(&pool, Uuid::new_v4(), "kept@example.org", "Kept Person").await;

    // A workspace only the user owns, with content, named after the user the
    // way a personal workspace is.
    let solo = Uuid::new_v4();
    sqlx::query("INSERT INTO workspaces (id, kind, name, slug) VALUES ($1, 'personal', $2, $3)")
        .bind(solo)
        .bind("Gone Person")
        .bind("gone-person")
        .execute(&pool)
        .await
        .unwrap();
    let solo = WorkspaceId::new(solo);
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

    // Every table that can hold the account's personal data: the users row,
    // the sealed provider payload in a session, the invitation addresses and
    // the legacy-identity audit metadata (the label and its provenance note).
    make_session(
        &pool,
        user,
        "sealed provider session for Gone Person gone@example.org user_1",
    )
    .await;
    let pending_invitation = make_invitation(&pool, shared, "gone@example.org", "pending").await;
    let accepted_invitation = make_invitation(&pool, shared, "gone@example.org", "accepted").await;
    let operator = make_user(&pool, Uuid::new_v4(), "operator@example.org", "Operator").await;
    sqlx::query(
        "INSERT INTO admin_audit_events
           (id, event_type, principal_kind, principal_user_id, target_user_id, metadata)
         VALUES ($1, 'legacy_actor.mapped', 'user', $2, $3,
                 jsonb_build_object(
                   'legacy_actor_kind', 'user',
                   'legacy_actor_id', $4::text,
                   'source', $5::text))",
    )
    .bind(Uuid::new_v4())
    .bind(operator.as_uuid())
    .bind(user.as_uuid())
    .bind("Gone Person (gone@example.org)")
    .bind("2019 screening export for Gone Person")
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
    assert_eq!(
        report.workspaces_left_ownerless,
        vec![shared.as_uuid()],
        "the shared workspace is kept and reported as left ownerless"
    );

    // The account is deactivated and anonymized; the WorkOS link is a
    // tombstone and `deleted_at` is the durable marker.
    let (status, workos, verified, deleted) = account_row(&pool, user).await;
    assert_eq!(status, "deactivated");
    assert!(verified.is_none());
    assert!(deleted);
    assert_eq!(
        workos,
        workos_tombstone(user),
        "the WorkOS link is tombstoned"
    );

    // Sessions are gone entirely: they held the sealed provider payload.
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM user_sessions WHERE user_id = $1")
            .bind(user.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );

    // Invitations: the pending one is deleted, the historical one keeps its
    // row under a tombstone address.
    assert!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM workspace_invitations WHERE id = $1")
            .bind(pending_invitation)
            .fetch_one(&pool)
            .await
            .unwrap()
            == 0
    );
    let kept =
        sqlx::query_scalar::<_, String>("SELECT email FROM workspace_invitations WHERE id = $1")
            .bind(accepted_invitation)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        kept,
        format!("deleted-{accepted_invitation}@invalid"),
        "the historical address is replaced, the row and its id stay"
    );

    // Audit metadata keeps the event, its ids and its type; the two named PII
    // keys move to the tombstone.
    let metadata = sqlx::query(
        "SELECT metadata FROM admin_audit_events
         WHERE event_type = 'legacy_actor.mapped' AND target_user_id = $1",
    )
    .bind(user.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let metadata: serde_json::Value = metadata.get("metadata");
    assert_eq!(metadata.get("legacy_actor_kind").unwrap(), "user");
    for key in PII_METADATA_KEYS {
        assert_eq!(
            metadata.get(key).and_then(serde_json::Value::as_str),
            Some(REDACTED),
            "{key} is redacted"
        );
    }

    // The solely owned workspace and its content are gone through the existing
    // deletions; the shared one and its contribution stay.
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

    // The audit rows of the completion, the deletion-proof record, and the
    // request's terminal state.
    assert_eq!(events(&pool, "account.deletion_completed").await, 1);
    assert_eq!(events(&pool, "project.deleted").await, 1);
    assert_eq!(events(&pool, "workspace.deleted").await, 1);
    assert_eq!(
        request_state(&pool, user).await.as_deref(),
        Some("completed"),
        "the request is never due again"
    );
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

    // Nothing anywhere still carries the email, the name or the WorkOS id.
    let leaks = personal_data_leaks(
        &pool,
        &[
            "gone@example.org",
            "Gone Person",
            format!("user_{}", user.as_uuid()).as_str(),
        ],
    )
    .await;
    assert!(leaks.is_empty(), "personal data survived: {leaks:#?}");
}

#[tokio::test]
async fn no_row_anywhere_holds_the_personal_data_after_completion() {
    let Some(pool) = database().await else {
        return;
    };
    let user = make_user(&pool, Uuid::new_v4(), "erased@example.org", "Erased Person").await;
    let other = make_user(&pool, Uuid::new_v4(), "other@example.org", "Other Person").await;
    let shared = make_workspace(&pool, Uuid::new_v4(), "shared-workspace").await;
    add_member(&pool, shared, user, "owner").await;
    add_member(&pool, shared, other, "member").await;
    make_session(
        &pool,
        user,
        "sealed payload Erased Person erased@example.org erased workos id",
    )
    .await;
    make_invitation(&pool, shared, "erased@example.org", "pending").await;
    make_invitation(&pool, shared, "erased@example.org", "accepted").await;
    make_invitation(&pool, shared, "erased@example.org", "expired").await;
    make_invitation(&pool, shared, "erased@example.org", "revoked").await;
    sqlx::query(
        "INSERT INTO admin_audit_events
           (id, event_type, principal_kind, principal_user_id, target_user_id, metadata)
         VALUES ($1, 'legacy_actor.non_identity_recorded', 'user', $2, $3,
                 jsonb_build_object(
                   'legacy_actor_kind', 'user',
                   'legacy_actor_id', 'Erased Person erased@example.org',
                   'source', 'legacy spreadsheet naming Erased Person'))",
    )
    .bind(Uuid::new_v4())
    .bind(other.as_uuid())
    .bind(user.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    request_account_deletion(&pool, user, AUDIT).await.unwrap();
    age_request(&pool, user, 31).await;
    complete_account_deletions(&pool, AUDIT).await.unwrap();

    let leaks = personal_data_leaks(&pool, &["erased@example.org", "Erased Person"]).await;
    assert!(leaks.is_empty(), "personal data survived: {leaks:#?}");
    // The colleague's data is untouched by the erasure.
    let untouched = personal_data_leaks(&pool, &["other@example.org", "Other Person"]).await;
    assert!(
        !untouched.is_empty(),
        "the scan must find the colleague's data in the users row, or it proves nothing"
    );
}

#[tokio::test]
async fn cancellation_and_completion_cannot_both_win() {
    let Some(pool) = database().await else {
        return;
    };

    // Way one: the cancellation wins the race. Nothing is destroyed and the
    // account keeps working.
    let cancelled = make_user(&pool, Uuid::new_v4(), "late@example.org", "Late Person").await;
    let workspace = make_workspace(&pool, Uuid::new_v4(), "late-workspace").await;
    add_member(&pool, workspace, cancelled, "owner").await;
    request_account_deletion(&pool, cancelled, AUDIT)
        .await
        .unwrap();
    age_request(&pool, cancelled, 31).await;
    // The cancellation arrives before the completion claims the request.
    assert_eq!(
        cancel_account_deletion(&pool, cancelled, AUDIT)
            .await
            .unwrap(),
        DeletionCancellationOutcome::Cancelled
    );
    let report = complete_account_deletions(&pool, AUDIT).await.unwrap();
    assert_eq!(report, Default::default(), "a cancelled request is not due");
    let row = sqlx::query("SELECT status, primary_email FROM users WHERE id = $1")
        .bind(cancelled.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("status"), "active");
    assert_eq!(row.get::<String, _>("primary_email"), "late@example.org");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM workspaces WHERE id = $1")
            .bind(workspace.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(events(&pool, "account.deletion_completed").await, 0);

    // Way two: the completion wins the race. The cancellation is refused with
    // a clear error instead of pretending it can still stop the deletion.
    request_account_deletion(&pool, cancelled, AUDIT)
        .await
        .unwrap();
    age_request(&pool, cancelled, 31).await;
    let report = complete_account_deletions(&pool, AUDIT).await.unwrap();
    assert_eq!(report.completed, 1);
    let refused = cancel_account_deletion(&pool, cancelled, AUDIT)
        .await
        .unwrap_err();
    assert!(
        matches!(refused, AccountDeletionError::AlreadyCompleted),
        "{refused}"
    );

    // The same refusal while the completion is in flight: the request is
    // claimed (`completing`) and the run was interrupted.
    let inflight = make_user(&pool, Uuid::new_v4(), "inflight@example.org", "In Flight").await;
    request_account_deletion(&pool, inflight, AUDIT)
        .await
        .unwrap();
    age_request(&pool, inflight, 31).await;
    sqlx::query(
        "UPDATE account_deletion_requests SET state = 'completing'
         WHERE user_id = $1 AND state = 'requested'",
    )
    .bind(inflight.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let refused = cancel_account_deletion(&pool, inflight, AUDIT)
        .await
        .unwrap_err();
    assert!(
        matches!(refused, AccountDeletionError::CompletionInFlight),
        "{refused}"
    );
    // And the interrupted completion is resumed by the next run.
    let report = complete_account_deletions(&pool, AUDIT).await.unwrap();
    assert_eq!(report.completed, 1);
    assert_eq!(
        request_state(&pool, inflight).await.as_deref(),
        Some("completed")
    );
}

#[tokio::test]
async fn a_member_joining_between_selection_and_deletion_keeps_the_workspace() {
    let Some(pool) = database().await else {
        return;
    };
    let user = make_user(&pool, Uuid::new_v4(), "racing@example.org", "Racing Person").await;
    let joiner = make_user(&pool, Uuid::new_v4(), "joiner@example.org", "Joiner").await;
    let workspace = make_workspace(&pool, Uuid::new_v4(), "raced-workspace").await;
    add_member(&pool, workspace, user, "owner").await;
    let project = Uuid::new_v4();
    make_project(&pool, project, workspace, "raced project").await;

    // The candidate scan names the workspace for destruction ...
    let candidates = workspace_candidates(&pool, user).await.unwrap();
    assert_eq!(candidates, vec![workspace]);
    // ... and a member joins before the per-workspace step runs.
    add_member(&pool, workspace, joiner, "member").await;

    let outcome = complete_workspace(&pool, user, workspace, AUDIT)
        .await
        .unwrap();
    assert!(
        matches!(outcome, WorkspaceOutcome::MembershipRemoved { .. }),
        "the workspace is kept and only this account's membership is removed: {outcome:?}"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM workspaces WHERE id = $1")
            .bind(workspace.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM projects WHERE id = $1")
            .bind(project)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1,
        "the content survives with the workspace"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM workspace_memberships WHERE workspace_id = $1"
        )
        .bind(workspace.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap(),
        1,
        "only this account's membership is removed"
    );
}

#[tokio::test]
async fn a_sole_non_owner_membership_never_deletes_a_workspace() {
    let Some(pool) = database().await else {
        return;
    };
    let user = make_user(&pool, Uuid::new_v4(), "member@example.org", "Member Person").await;
    let workspace = make_workspace(&pool, Uuid::new_v4(), "member-workspace").await;
    add_member(&pool, workspace, user, "member").await;
    let project = Uuid::new_v4();
    make_project(&pool, project, workspace, "member project").await;

    request_account_deletion(&pool, user, AUDIT).await.unwrap();
    age_request(&pool, user, 31).await;
    let report = complete_account_deletions(&pool, AUDIT).await.unwrap();
    assert_eq!(report.workspaces_deleted, 0, "not an owner: never deleted");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM workspaces WHERE id = $1")
            .bind(workspace.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM projects WHERE id = $1")
            .bind(project)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    // The membership is withdrawn and the ownerless workspace is reported.
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM workspace_memberships WHERE workspace_id = $1"
        )
        .bind(workspace.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        report.workspaces_left_ownerless,
        vec![workspace.as_uuid()],
        "the operator recovery flow is told about it"
    );
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

#[tokio::test]
async fn n_1_sign_in_sql_cannot_resurrect_a_deleted_account() {
    let Some(pool) = database().await else {
        return;
    };
    let mut id = Uuid::new_v4();
    let user = make_user(&pool, id, "gone@example.org", "Gone Person").await;
    // The provider identity is known to WorkOS and to N-1 SQL.
    let external = "workos_user_42";
    sqlx::query("UPDATE users SET workos_user_id = $2 WHERE id = $1")
        .bind(id)
        .bind(external)
        .execute(&pool)
        .await
        .unwrap();
    request_account_deletion(&pool, user, AUDIT).await.unwrap();
    age_request(&pool, user, 31).await;
    complete_account_deletions(&pool, AUDIT).await.unwrap();

    // Even once the five-year compliance log is gone, the marker stands.
    sqlx::query("DELETE FROM fulfilled_account_deletions WHERE user_id = $1")
        .bind(user.as_uuid())
        .execute(&pool)
        .await
        .unwrap();

    // The N-1 sign-in SQL: insert-or-nothing, then the profile update, then
    // the read that decides what the request sees.
    id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, workos_user_id, primary_email, verified_email, display_name, status)
         VALUES ($1, $2, $3, $3, $4, 'active')
         ON CONFLICT (workos_user_id) DO NOTHING",
    )
    .bind(id)
    .bind(external)
    .bind("gone@example.org")
    .bind("Gone Person")
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE users
         SET primary_email = $2, verified_email = $2, display_name = $3, updated_at = now()
         WHERE workos_user_id = $1",
    )
    .bind(external)
    .bind("gone@example.org")
    .bind("Gone Person")
    .execute(&pool)
    .await
    .unwrap();
    let row = sqlx::query(
        "SELECT id, status, primary_email, display_name FROM users WHERE workos_user_id = $1",
    )
    .bind(external)
    .fetch_one(&pool)
    .await
    .unwrap();

    // The sign-in creates a NEW user: that is the expected re-signup, and the
    // old account is not resurrected by it.
    assert_ne!(row.get::<Uuid, _>("id"), user.as_uuid());
    assert_eq!(row.get::<String, _>("status"), "active");
    assert_eq!(row.get::<String, _>("primary_email"), "gone@example.org");
    assert_eq!(row.get::<String, _>("display_name"), "Gone Person");

    // The deleted account keeps its anonymized form, marker and tombstone.
    let (status, workos, verified, deleted) = account_row(&pool, user).await;
    assert_eq!(status, "deactivated");
    assert!(deleted);
    assert!(verified.is_none());
    assert_eq!(workos, workos_tombstone(user));

    // Even a write aimed straight at the old row is turned into the anonymized
    // form by the database boundary guard (migration 0111).
    sqlx::query(
        "UPDATE users
         SET status = 'active', primary_email = 'gone@example.org',
             verified_email = 'gone@example.org', display_name = 'Gone Person',
             deleted_at = NULL
         WHERE id = $1",
    )
    .bind(user.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let (status, workos, verified, deleted) = account_row(&pool, user).await;
    assert_eq!(status, "deactivated", "the account cannot be reactivated");
    assert!(deleted, "the marker can never be cleared");
    assert!(verified.is_none());
    assert_eq!(workos, workos_tombstone(user));
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
    // Nothing is due any more, and the request is terminal.
    let second = complete_account_deletions(&pool, AUDIT).await.unwrap();
    assert_eq!(second.completed, 0);
    assert_eq!(
        second.re_applied, 0,
        "every logged deletion is replayed every run; a replay that finds nothing to undo changes nothing"
    );
    assert_eq!(events(&pool, "account.deletion_completed").await, 1);
    assert_eq!(
        request_state(&pool, user).await.as_deref(),
        Some("completed"),
        "the request is never due again, whatever the proof row does"
    );

    // A restore brings the personal data back (docs/security.md, "Restore
    // step"). A backup restore reloads tuples without firing triggers, so the
    // guard is disabled for the reload exactly like a COPY would leave it.
    sqlx::query("ALTER TABLE users DISABLE TRIGGER users_anonymized_account_guard")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE users
         SET status = 'active', primary_email = 'restored@example.org',
             verified_email = 'restored@example.org', display_name = 'Restored Person',
             workos_user_id = 'user_restored', deleted_at = NULL
         WHERE id = $1",
    )
    .bind(user.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("ALTER TABLE users ENABLE TRIGGER users_anonymized_account_guard")
        .execute(&pool)
        .await
        .unwrap();
    let restored_workspace = make_workspace(&pool, Uuid::new_v4(), "restored-workspace").await;
    add_member(&pool, restored_workspace, user, "owner").await;
    make_session(
        &pool,
        user,
        "sealed payload Restored Person restored@example.org",
    )
    .await;
    let restored_invitation =
        make_invitation(&pool, restored_workspace, "restored@example.org", "pending").await;

    let third = complete_account_deletions(&pool, AUDIT).await.unwrap();
    assert_eq!(third.completed, 0);
    assert_eq!(third.re_applied, 1);
    assert_eq!(third.workspaces_deleted, 1);
    let (status, _, verified, _) = account_row(&pool, user).await;
    assert_eq!(status, "deactivated");
    assert!(verified.is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT (SELECT count(*) FROM user_sessions WHERE user_id = $1)
                  + (SELECT count(*) FROM workspace_invitations WHERE id = $2)"
        )
        .bind(user.as_uuid())
        .bind(restored_invitation)
        .fetch_one(&pool)
        .await
        .unwrap(),
        0,
        "the restored session and invitation go again"
    );
    // Re-applying is not a second deletion: the completion record stands once.
    assert_eq!(events(&pool, "account.deletion_completed").await, 1);
}

#[tokio::test]
async fn the_replay_reapplies_cleanup_even_when_the_profile_is_unchanged() {
    let Some(pool) = database().await else {
        return;
    };
    let user = make_user(&pool, Uuid::new_v4(), "replay@example.org", "Replay Person").await;
    let other = make_user(&pool, Uuid::new_v4(), "other@example.org", "Other Person").await;
    let shared = make_workspace(&pool, Uuid::new_v4(), "shared-workspace").await;
    add_member(&pool, shared, other, "member").await;

    request_account_deletion(&pool, user, AUDIT).await.unwrap();
    age_request(&pool, user, 31).await;
    complete_account_deletions(&pool, AUDIT).await.unwrap();

    // A restore brings back only auxiliary rows: a session and a recreated
    // solely-owned workspace (invitation addresses are replayed from the
    // restored users row; see the restore test). The users row itself is still anonymized, so a replay that
    // only looked at profile differences would skip all of it.
    make_session(
        &pool,
        user,
        "sealed payload Replay Person replay@example.org",
    )
    .await;
    let recreated = make_workspace(&pool, Uuid::new_v4(), "recreated-workspace").await;
    add_member(&pool, recreated, user, "owner").await;
    let project = Uuid::new_v4();
    make_project(&pool, project, recreated, "recreated project").await;

    let report = complete_account_deletions(&pool, AUDIT).await.unwrap();
    assert_eq!(report.completed, 0);
    assert_eq!(report.re_applied, 1, "the logged deletion is replayed");
    assert_eq!(report.workspaces_deleted, 1);
    assert_eq!(report.projects_deleted, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM user_sessions WHERE user_id = $1")
            .bind(user.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap(),
        0,
        "the restored session goes again"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM workspaces WHERE id = $1")
            .bind(recreated.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap(),
        0,
        "the recreated workspace goes again"
    );
    // Still one deletion, recorded once.
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
    assert_eq!(
        request_state(&pool, user).await.as_deref(),
        Some("requested")
    );
    assert_eq!(events(&pool, "account.deletion_completed").await, 0);
}

#[tokio::test]
async fn the_audit_redaction_gate_is_narrow_and_scoped_to_one_user() {
    let Some(pool) = database().await else {
        return;
    };
    let user = make_user(&pool, Uuid::new_v4(), "labelled@example.org", "Labelled").await;
    let other = make_user(&pool, Uuid::new_v4(), "other@example.org", "Other").await;
    let event = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO admin_audit_events
           (id, event_type, principal_kind, principal_user_id, target_user_id, metadata)
         VALUES ($1, 'legacy_actor.mapped', 'user', $2, $3,
                 jsonb_build_object(
                   'legacy_actor_kind', 'user',
                   'legacy_actor_id', 'Labelled labelled@example.org',
                   'source', 'legacy spreadsheet'))",
    )
    .bind(event)
    .bind(other.as_uuid())
    .bind(user.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    // Without the gate every UPDATE is still refused.
    let refused = sqlx::query(
        "UPDATE admin_audit_events
         SET metadata = jsonb_build_object(
               'legacy_actor_kind', 'user', 'legacy_actor_id', 'redacted', 'source', 'redacted')
         WHERE id = $1",
    )
    .bind(event)
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(refused.to_string().contains("append-only"), "{refused}");

    // The gate names one user; another user's rows stay closed.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('deepref.audit_redact', $1, true)")
        .bind(other.as_uuid().to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let refused = sqlx::query(
        "UPDATE admin_audit_events
         SET metadata = metadata || jsonb_build_object(
               'legacy_actor_id', 'redacted', 'source', 'redacted')
         WHERE id = $1",
    )
    .bind(event)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(refused.to_string().contains("append-only"), "{refused}");
    tx.rollback().await.unwrap();

    // Even for the right user, only the named keys may move: an identifier
    // change is refused.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('deepref.audit_redact', $1, true)")
        .bind(user.as_uuid().to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let refused = sqlx::query(
        "UPDATE admin_audit_events
         SET metadata = jsonb_build_object(
               'legacy_actor_kind', 'user', 'legacy_actor_id', 'redacted', 'source', 'redacted'),
             target_user_id = NULL
         WHERE id = $1",
    )
    .bind(event)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(refused.to_string().contains("append-only"), "{refused}");
    tx.rollback().await.unwrap();
    // The narrow redaction itself is allowed.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('deepref.audit_redact', $1, true)")
        .bind(user.as_uuid().to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE admin_audit_events
         SET metadata = metadata || jsonb_build_object(
               'legacy_actor_id', 'redacted', 'source', 'redacted')
         WHERE id = $1",
    )
    .bind(event)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // And the gate does not outlive its transaction.
    let refused = sqlx::query(
        "UPDATE admin_audit_events
         SET metadata = metadata || jsonb_build_object(
               'legacy_actor_id', 'redacted', 'source', 'redacted')
         WHERE id = $1",
    )
    .bind(event)
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(refused.to_string().contains("append-only"), "{refused}");

    let row = sqlx::query(
        "SELECT event_type, target_user_id, metadata FROM admin_audit_events WHERE id = $1",
    )
    .bind(event)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("event_type"), "legacy_actor.mapped");
    assert_eq!(row.get::<Uuid, _>("target_user_id"), user.as_uuid());
    let metadata: serde_json::Value = row.get("metadata");
    assert_eq!(
        metadata
            .get("legacy_actor_id")
            .and_then(serde_json::Value::as_str),
        Some(REDACTED)
    );
    assert_eq!(
        metadata.get("source").and_then(serde_json::Value::as_str),
        Some(REDACTED)
    );
    assert_eq!(metadata.get("legacy_actor_kind").unwrap(), "user");
}

/// The WorkOS link tombstone is what makes a later sign-in a NEW account: no
/// provider identity can match it, and the unique constraint still holds.
#[test]
fn the_workos_tombstone_is_stable_and_unmatchable() {
    let user = UserId::new(Uuid::new_v4());
    assert_eq!(
        workos_tombstone(user),
        format!("deleted:{}", user.as_uuid())
    );
    assert_ne!(workos_tombstone(user), user.as_uuid().to_string());
}
