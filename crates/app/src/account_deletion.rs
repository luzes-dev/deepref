//! "Delete my account" (handoff 11, BUSINESS_DECISIONS).
//!
//! A user schedules their own deletion (`POST /me/deletion-request`), may
//! cancel it for 30 days, and `deepref-server accounts complete-deletions`
//! completes it afterwards. Completion:
//!
//! * deactivates the account and anonymizes the PII (email and display name
//!   become the "Former member" label, `verified_email` is cleared), keeping
//!   the [`UserId`] stable;
//! * keeps every scientific provenance reference and the whole audit history;
//! * deletes the workspaces the user solely owns and their content through the
//!   existing project and workspace deletions, unless other members remain;
//! * keeps contributions in shared workspaces, attributed to the anonymized
//!   user;
//! * writes the audit rows and the deletion-proof `account.deletion_completed`
//!   row, and logs the fulfillment in `fulfilled_account_deletions` (user id
//!   and time only) so restores can re-apply the deletion idempotently.

use deepref_domain::{ProjectId, UserId, WorkspaceId};
use serde_json::json;
use sqlx::{PgPool, Row};
use thiserror::Error;
use uuid::Uuid;

use crate::identity::{AuditPrincipal, AuditRequest, insert_admin_audit};
use crate::project_deletion::delete_project_in_transaction;
use crate::workspaces::{MIGRATION_WORKSPACE_ID, WorkspaceError, delete_workspace};

/// What the anonymized account keeps showing where a name is due
/// (BUSINESS_DECISIONS: contributions in shared workspaces stay "under an
/// anonymized 'former member' label").
pub const FORMER_MEMBER_LABEL: &str = "Former member";

/// How long a deletion request waits before completion (BUSINESS_DECISIONS:
/// "deleted 30 days after a deletion request").
pub const DELETION_GRACE_PERIOD: &str = "30 days";

#[derive(Debug, Error)]
pub enum AccountDeletionError {
    #[error("account deletion storage failed")]
    Database(#[from] sqlx::Error),
    #[error("no such user")]
    UnknownUser,
    #[error("the account is not active")]
    Inactive,
    #[error("cleaning up the user's workspaces failed")]
    Workspace(#[from] WorkspaceError),
}

/// What a deletion request did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionRequestOutcome {
    /// The request is recorded; completion runs in 30 days.
    Requested,
    /// A request was already pending; nothing changed and nothing was audited.
    AlreadyPending,
}

/// What a cancellation did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionCancellationOutcome {
    /// The pending request is cancelled and audited.
    Cancelled,
    /// No request was pending; nothing changed and nothing was audited.
    NoRequest,
}

/// Records that the user wants their account deleted. Idempotent: a replay
/// changes nothing and writes no second `account.deletion_requested` row.
pub async fn request_account_deletion(
    pool: &PgPool,
    user: UserId,
    audit: AuditRequest<'_>,
) -> Result<DeletionRequestOutcome, AccountDeletionError> {
    let mut tx = pool.begin().await?;
    let requested: Option<Uuid> = sqlx::query_scalar(
        "UPDATE users
         SET deletion_requested_at = now(), updated_at = now()
         WHERE id = $1 AND status = 'active' AND deletion_requested_at IS NULL
         RETURNING id",
    )
    .bind(user.as_uuid())
    .fetch_optional(&mut *tx)
    .await?;
    if requested.is_some() {
        insert_admin_audit(
            &mut tx,
            "account.deletion_requested",
            AuditPrincipal::User(user),
            Some(user),
            None,
            None,
            json!({}),
            audit,
        )
        .await?;
        tx.commit().await?;
        return Ok(DeletionRequestOutcome::Requested);
    }
    let row = sqlx::query("SELECT status, deletion_requested_at FROM users WHERE id = $1")
        .bind(user.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
    tx.commit().await?;
    let Some(row) = row else {
        return Err(AccountDeletionError::UnknownUser);
    };
    let pending: Option<chrono::DateTime<chrono::Utc>> = row.try_get("deletion_requested_at")?;
    if pending.is_some() {
        return Ok(DeletionRequestOutcome::AlreadyPending);
    }
    let status: String = row.try_get("status")?;
    if status == "active" {
        // Another request won the race and recorded the same state.
        return Ok(DeletionRequestOutcome::AlreadyPending);
    }
    Err(AccountDeletionError::Inactive)
}

/// Cancels a pending deletion request. Idempotent: with no request pending
/// nothing changes and nothing is audited.
pub async fn cancel_account_deletion(
    pool: &PgPool,
    user: UserId,
    audit: AuditRequest<'_>,
) -> Result<DeletionCancellationOutcome, AccountDeletionError> {
    let mut tx = pool.begin().await?;
    let cancelled: Option<Uuid> = sqlx::query_scalar(
        "UPDATE users
         SET deletion_requested_at = NULL, updated_at = now()
         WHERE id = $1 AND deletion_requested_at IS NOT NULL
         RETURNING id",
    )
    .bind(user.as_uuid())
    .fetch_optional(&mut *tx)
    .await?;
    if cancelled.is_some() {
        insert_admin_audit(
            &mut tx,
            "account.deletion_cancelled",
            AuditPrincipal::User(user),
            Some(user),
            None,
            None,
            json!({}),
            audit,
        )
        .await?;
        tx.commit().await?;
        return Ok(DeletionCancellationOutcome::Cancelled);
    }
    let known: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE id = $1")
        .bind(user.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
    tx.commit().await?;
    if known.is_none() {
        return Err(AccountDeletionError::UnknownUser);
    }
    Ok(DeletionCancellationOutcome::NoRequest)
}

/// What one `deepref-server accounts complete-deletions` run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct CompletionReport {
    /// Accounts completed for the first time.
    pub completed: u64,
    /// Already-logged deletions re-applied (a restore brought the row back).
    pub re_applied: u64,
    pub workspaces_deleted: u64,
    pub projects_deleted: u64,
}

/// Completes every deletion request that has aged out its 30 days, and
/// re-applies every already-fulfilled deletion whose account a restore
/// brought back. Safe to interrupt and rerun: the fulfillment log is written
/// in the same transaction as the anonymization, and every step before it is
/// idempotent.
///
/// The audit rows are written only when a deletion is fulfilled for the first
/// time; a re-application of the same logged deletion is a restore remedy, not
/// a second deletion (docs/security.md, "Restore step").
pub async fn complete_account_deletions(
    pool: &PgPool,
    audit: AuditRequest<'_>,
) -> Result<CompletionReport, AccountDeletionError> {
    let mut report = CompletionReport::default();
    for account in due_accounts(pool).await? {
        let (workspaces, projects) =
            delete_solely_owned_workspaces(pool, account.user_id, audit).await?;
        report.workspaces_deleted += workspaces;
        report.projects_deleted += projects;
        if finalize_account(pool, account, audit).await? {
            report.completed += 1;
        } else {
            report.re_applied += 1;
        }
    }
    Ok(report)
}

#[derive(Debug, Clone, Copy)]
struct DueAccount {
    user_id: UserId,
}

/// Accounts the command owes work: a request past its 30 days that has never
/// been fulfilled, or a fulfilled deletion whose account no longer holds its
/// anonymized form.
async fn due_accounts(pool: &PgPool) -> Result<Vec<DueAccount>, AccountDeletionError> {
    let rows = sqlx::query(
        "SELECT u.id
         FROM users u
         LEFT JOIN fulfilled_account_deletions f ON f.user_id = u.id
         WHERE (u.deletion_requested_at IS NOT NULL
                AND u.deletion_requested_at <= now() - interval '30 days'
                AND f.user_id IS NULL)
            OR (f.user_id IS NOT NULL
                AND (u.status <> 'deactivated'
                     OR u.verified_email IS NOT NULL
                     OR u.primary_email IS DISTINCT FROM $1
                     OR u.display_name IS DISTINCT FROM $1))
         ORDER BY u.deletion_requested_at NULLS LAST, u.id",
    )
    .bind(FORMER_MEMBER_LABEL)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(DueAccount {
                user_id: UserId::new(row.try_get("id")?),
            })
        })
        .collect()
}

/// Deletes the workspaces no other member remains in, and their content,
/// through the existing project and workspace deletions (which write the
/// `project.deleted`, `membership.removed`, `invitation.revoked` and
/// `workspace.deleted` audit rows). Workspaces other members remain in are
/// kept; the contributions stay attributed to the (still then) account.
async fn delete_solely_owned_workspaces(
    pool: &PgPool,
    user: UserId,
    audit: AuditRequest<'_>,
) -> Result<(u64, u64), AccountDeletionError> {
    let workspace_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT m.workspace_id
         FROM workspace_memberships m
         WHERE m.user_id = $1
           AND m.workspace_id <> $2
           AND NOT EXISTS (
             SELECT 1 FROM workspace_memberships o
             WHERE o.workspace_id = m.workspace_id AND o.user_id <> $1)
         ORDER BY m.created_at, m.id",
    )
    .bind(user.as_uuid())
    .bind(MIGRATION_WORKSPACE_ID.as_uuid())
    .fetch_all(pool)
    .await?;
    let mut workspaces = 0;
    let mut projects = 0;
    for workspace_id in workspace_ids {
        let workspace = WorkspaceId::new(workspace_id);
        let mut tx = pool.begin().await?;
        let project_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM projects WHERE workspace_id = $1 ORDER BY created_at, id",
        )
        .bind(workspace_id)
        .fetch_all(&mut *tx)
        .await?;
        for project_id in project_ids {
            let project = ProjectId::new(project_id);
            if delete_project_in_transaction(&mut tx, project_id).await? {
                crate::project_audit::record_project_deleted(
                    &mut tx,
                    Some(user),
                    workspace,
                    project,
                    audit,
                )
                .await?;
                projects += 1;
            }
        }
        tx.commit().await?;
        delete_workspace(pool, user, workspace, audit).await?;
        workspaces += 1;
    }
    Ok((workspaces, projects))
}

/// Anonymizes and deactivates one account, revokes its sessions, writes the
/// audit rows and records the fulfillment. Returns whether this run fulfilled
/// the deletion for the first time.
async fn finalize_account(
    pool: &PgPool,
    account: DueAccount,
    audit: AuditRequest<'_>,
) -> Result<bool, AccountDeletionError> {
    let user = account.user_id;
    let mut tx = pool.begin().await?;
    let locked: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE id = $1 FOR UPDATE")
        .bind(user.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
    if locked.is_none() {
        return Err(AccountDeletionError::UnknownUser);
    }
    let newly_fulfilled = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO fulfilled_account_deletions (user_id) VALUES ($1)
         ON CONFLICT (user_id) DO NOTHING RETURNING user_id",
    )
    .bind(user.as_uuid())
    .fetch_optional(&mut *tx)
    .await?
    .is_some();
    // The PII goes; the user id and everything that references it stay.
    sqlx::query(
        "UPDATE users
         SET status = 'deactivated',
             primary_email = $2,
             display_name = $2,
             verified_email = NULL,
             deleted_at = coalesce(deleted_at, now()),
             updated_at = now()
         WHERE id = $1",
    )
    .bind(user.as_uuid())
    .bind(FORMER_MEMBER_LABEL)
    .execute(&mut *tx)
    .await?;
    let session_refs: Vec<Vec<u8>> = sqlx::query_scalar(
        "UPDATE user_sessions
         SET revoked_at = coalesce(revoked_at, now())
         WHERE user_id = $1 AND revoked_at IS NULL
         RETURNING session_ref",
    )
    .bind(user.as_uuid())
    .fetch_all(&mut *tx)
    .await?;
    if newly_fulfilled {
        for session_ref in &session_refs {
            insert_admin_audit(
                &mut tx,
                "auth.session_revoked",
                AuditPrincipal::System,
                Some(user),
                None,
                None,
                json!({ "reason": "user_deactivated" }),
                AuditRequest {
                    request_id: audit.request_id,
                    session_ref: Some(session_ref),
                },
            )
            .await?;
        }
        insert_admin_audit(
            &mut tx,
            "user.deactivated",
            AuditPrincipal::System,
            Some(user),
            None,
            None,
            json!({ "source": "account_deletion" }),
            audit,
        )
        .await?;
        // The deletion-proof compliance record (BUSINESS_DECISIONS: kept five
        // years, no content).
        insert_admin_audit(
            &mut tx,
            "account.deletion_completed",
            AuditPrincipal::System,
            Some(user),
            None,
            None,
            json!({}),
            audit,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(newly_fulfilled)
}
