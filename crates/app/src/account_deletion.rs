//! "Delete my account" (handoff 11, BUSINESS_DECISIONS).
//!
//! A user schedules their own deletion (`POST /me/deletion-request`), may
//! cancel it for 30 days, and `deepref-server accounts complete-deletions`
//! completes it afterwards. Completion:
//!
//! * deactivates the account and erases the PII: email and display name become
//!   the "Former member" label, `verified_email` is cleared and the WorkOS link
//!   is tombstoned (`deleted:<user uuid>`), keeping the [`UserId`] stable;
//! * deletes the workspaces the account solely owns and their content, keeps
//!   the other ones and only withdraws this account's membership from them;
//! * keeps every scientific provenance reference and the whole audit history;
//! * deletes the session rows (they hold the sealed provider payload),
//!   scrubs invitation addresses from persisted digests, redacts native
//!   legacy-decision sources, and redacts the PII in audit metadata;
//! * writes the audit rows and the deletion-proof `account.deletion_completed`
//!   row, logs the fulfillment in `fulfilled_account_deletions` (user id, time,
//!   and normalized-address SHA-256 digests, never plaintext email), and moves
//!   the request to its terminal `completed` state.
//!
//! # State machine and locking
//!
//! The request is an explicit state machine (migration 0111):
//!
//! ```text
//! requested --(cancel)--> cancelled        (terminal; a new request is a new row)
//! requested --(claim)-->  completing --> completed   (terminal)
//! ```
//!
//! Completion and cancellation both take the same two row locks in the same
//! fixed order: **the request row first, then the user row**. Cancellation
//! succeeds only from `requested`; from `completing` or `completed` it fails
//! with a clear error. Completion claims a due request by re-checking
//! `state = 'requested' AND requested_at <= now() - 30 days` and moving the row
//! to `completing` in that transaction, *before* any destructive work, and the
//! transaction that finishes the account moves it to `completed` — so a
//! request is never due twice, whatever happens to the five-year proof row.
//!
//! A run that dies between the claim and the finish leaves the request in
//! `completing`; the next run resumes it (every step is idempotent and no
//! audit row is written twice).
//!
//! # Idempotence and restores
//!
//! Every already-logged deletion is replayed on every run, whatever the
//! profile fields look like: sessions, invitation addresses, solely-owned
//! workspaces recreated since, and the users row. The users row is additionally
//! protected at the database boundary by `users.deleted_at` and the trigger
//! from migration 0111, which covers N-1 SQL and restores independently of the
//! five-year `fulfilled_account_deletions` log.

use deepref_domain::{MembershipStatus, ProjectId, UserId, WorkspaceId, WorkspaceRole};
use serde::Serialize;
use serde_json::json;
use sqlx::{PgPool, Postgres, Row, Transaction};
use thiserror::Error;
use uuid::Uuid;

use crate::identity::{AuditPrincipal, AuditRequest, insert_admin_audit};
use crate::project_audit::record_project_deleted;
use crate::project_deletion::delete_project_in_transaction;
use crate::workspaces::{
    MIGRATION_WORKSPACE_ID, WorkspaceError, delete_workspace_rows, lock_workspace,
    normalize_invitation_emails, remove_membership_in_transaction, scrub_user_invitations,
};

/// What the anonymized account keeps showing where a name is due
/// (BUSINESS_DECISIONS: contributions in shared workspaces stay "under an
/// anonymized 'former member' label"). The database trigger of migration 0111
/// re-asserts exactly this value; the deletion tests assert the two agree.
pub const FORMER_MEMBER_LABEL: &str = "Former member";

/// How long a deletion request waits before completion (BUSINESS_DECISIONS:
/// "deleted 30 days after a deletion request").
pub const DELETION_GRACE_PERIOD: &str = "30 days";

/// The audit metadata keys that can carry a person's name or label (03c stores
/// the legacy actor label and the operator's provenance note there). Completion
/// redacts exactly these keys for the deleted account, and the append-only
/// trigger (migration 0112) refuses every other audit UPDATE.
pub const PII_METADATA_KEYS: [&str; 2] = ["legacy_actor_id", "source"];

/// The value the redacted keys take. The trigger allows no other value.
pub const REDACTED: &str = "redacted";

/// The WorkOS link of a completed deletion: a tombstone that keeps the unique
/// constraint and that no provider identity can ever match, so a later sign-in
/// by the same person resolves to a NEW user instead of resurrecting this one.
pub fn workos_tombstone(user: UserId) -> String {
    format!("deleted:{}", user.as_uuid())
}

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
    #[error("the account deletion is already being completed and can no longer be cancelled")]
    CompletionInFlight,
    #[error("the account deletion has already completed and can no longer be cancelled")]
    AlreadyCompleted,
}

/// The state one deletion request is in (migration 0111).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionRequestState {
    /// The user asked; the 30-day window runs from `requested_at`.
    Requested,
    /// The user changed their mind. Terminal: a fresh request is a new row
    /// whose 30-day wait starts again.
    Cancelled,
    /// An operator run claimed the request and is deleting content.
    Completing,
    /// Terminal: the deletion is done and the request is never due again.
    Completed,
}

impl DeletionRequestState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Cancelled => "cancelled",
            Self::Completing => "completing",
            Self::Completed => "completed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "requested" => Some(Self::Requested),
            "cancelled" => Some(Self::Cancelled),
            "completing" => Some(Self::Completing),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }
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
/// changes nothing and writes no second `account.deletion_requested` row. A
/// request after a cancellation is a new request row, so the 30-day wait
/// restarts.
pub async fn request_account_deletion(
    pool: &PgPool,
    user: UserId,
    audit: AuditRequest<'_>,
) -> Result<DeletionRequestOutcome, AccountDeletionError> {
    let mut tx = pool.begin().await?;
    // Creation locks only the user row: the request row it adds does not exist
    // yet, and the open-request unique index keeps one request per user.
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM users WHERE id = $1 FOR UPDATE")
            .bind(user.as_uuid())
            .fetch_optional(&mut *tx)
            .await?;
    let Some(status) = status else {
        return Err(AccountDeletionError::UnknownUser);
    };
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO account_deletion_requests (id, user_id, state, requested_at)
         SELECT $1, u.id, 'requested', now()
         FROM users u
         WHERE u.id = $2 AND u.status = 'active'
           AND NOT EXISTS (
             SELECT 1 FROM account_deletion_requests r
             WHERE r.user_id = u.id AND r.state IN ('requested', 'completing'))
         ON CONFLICT DO NOTHING
         RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(user.as_uuid())
    .fetch_optional(&mut *tx)
    .await?;
    if inserted.is_some() {
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
    let pending: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM account_deletion_requests
         WHERE user_id = $1 AND state IN ('requested', 'completing')",
    )
    .bind(user.as_uuid())
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    if pending.is_some() {
        return Ok(DeletionRequestOutcome::AlreadyPending);
    }
    if status == "active" {
        // Another request won the race and recorded the same state.
        return Ok(DeletionRequestOutcome::AlreadyPending);
    }
    Err(AccountDeletionError::Inactive)
}

/// Cancels a pending deletion request. Idempotent: with no request pending
/// nothing changes and nothing is audited.
///
/// Cancellation takes the same two locks in the same order completion uses —
/// the request row first, then the user row — so it can never slip in between
/// a claim and the destructive work that follows it. It succeeds only from
/// [`DeletionRequestState::Requested`]; from `completing` or `completed` it
/// fails with a clear error instead of pretending the deletion can still be
/// stopped.
pub async fn cancel_account_deletion(
    pool: &PgPool,
    user: UserId,
    audit: AuditRequest<'_>,
) -> Result<DeletionCancellationOutcome, AccountDeletionError> {
    let mut tx = pool.begin().await?;
    let request = sqlx::query(
        "SELECT id, state FROM account_deletion_requests
         WHERE user_id = $1
         ORDER BY requested_at DESC, id DESC
         LIMIT 1
         FOR UPDATE",
    )
    .bind(user.as_uuid())
    .fetch_optional(&mut *tx)
    .await?;
    let known: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE id = $1 FOR UPDATE")
        .bind(user.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
    let Some(request) = request else {
        tx.commit().await?;
        if known.is_none() {
            return Err(AccountDeletionError::UnknownUser);
        }
        return Ok(DeletionCancellationOutcome::NoRequest);
    };
    let state = DeletionRequestState::parse(&request.try_get::<String, _>("state")?)
        .ok_or(AccountDeletionError::Database(sqlx::Error::RowNotFound))?;
    match state {
        DeletionRequestState::Completing => Err(AccountDeletionError::CompletionInFlight),
        DeletionRequestState::Completed => Err(AccountDeletionError::AlreadyCompleted),
        DeletionRequestState::Cancelled => {
            tx.commit().await?;
            Ok(DeletionCancellationOutcome::NoRequest)
        }
        DeletionRequestState::Requested => {
            let cancelled: Option<Uuid> = sqlx::query_scalar(
                "UPDATE account_deletion_requests
                 SET state = 'cancelled', resolved_at = now()
                 WHERE id = $1 AND state = 'requested'
                 RETURNING id",
            )
            .bind(request.try_get::<Uuid, _>("id")?)
            .fetch_optional(&mut *tx)
            .await?;
            if cancelled.is_none() {
                // The claim won the race between the read and the write.
                tx.commit().await?;
                return Err(AccountDeletionError::CompletionInFlight);
            }
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
            Ok(DeletionCancellationOutcome::Cancelled)
        }
    }
}

/// What one `deepref-server accounts complete-deletions` run did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CompletionReport {
    /// Accounts completed for the first time.
    pub completed: u64,
    /// Logged deletions whose cleanup was replayed (every run replays every
    /// logged deletion; a replay that finds nothing to undo changes nothing).
    pub re_applied: u64,
    pub workspaces_deleted: u64,
    pub projects_deleted: u64,
    /// Workspaces kept but left without an owner, for the operator recovery
    /// flow (03a leaves them ownerless rather than blocking the removal).
    pub workspaces_left_ownerless: Vec<Uuid>,
}

/// Completes every deletion request that has aged out its 30 days, and replays
/// the cleanup of every already-fulfilled deletion (a restore can bring rows
/// back). Safe to interrupt and rerun: the request is claimed before any
/// destructive work, the fulfillment log is written in the same transaction as
/// the anonymization, and every step before it is idempotent.
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
        if !claim_request(pool, &account, audit).await? {
            // Cancelled between the scan and the claim: nothing is destroyed.
            continue;
        }
        let mut changed = false;
        for workspace in workspace_candidates(pool, account.user_id).await? {
            match complete_workspace(pool, account.user_id, workspace, audit).await? {
                WorkspaceOutcome::Deleted { projects } => {
                    report.workspaces_deleted += 1;
                    report.projects_deleted += projects;
                    changed = true;
                }
                WorkspaceOutcome::MembershipRemoved { left_ownerless } => {
                    changed = true;
                    if left_ownerless {
                        report.workspaces_left_ownerless.push(workspace.as_uuid());
                    }
                }
                WorkspaceOutcome::AlreadyGone => {}
            }
        }
        let finalized = finalize_account(pool, &account, audit).await?;
        if finalized.newly_fulfilled {
            report.completed += 1;
        } else if changed || finalized.changed {
            // A replay that found something to undo. One that found nothing
            // changes nothing and is not counted.
            report.re_applied += 1;
        }
    }
    Ok(report)
}

/// One account the command owes work: a request past its 30 days (or an
/// interrupted claim), or a fulfilled deletion whose cleanup a restore undid.
#[derive(Debug, Clone, Copy)]
pub struct DueAccount {
    pub user_id: UserId,
    /// The open request being completed, when there is one.
    pub request_id: Option<Uuid>,
    /// The deletion was already fulfilled once; this run only replays cleanup.
    pub logged: bool,
}

/// Accounts the command owes work. Every logged deletion is replayed on every
/// run, whatever the profile fields look like (11 Part B): sessions,
/// invitations, recreated workspaces and the users row are cleaned again.
pub async fn due_accounts(pool: &PgPool) -> Result<Vec<DueAccount>, AccountDeletionError> {
    let rows = sqlx::query(
        "SELECT u.id AS user_id, r.id AS request_id, (f.user_id IS NOT NULL) AS logged
         FROM users u
         LEFT JOIN fulfilled_account_deletions f ON f.user_id = u.id
         LEFT JOIN account_deletion_requests r
           ON r.user_id = u.id AND r.state IN ('requested', 'completing')
         WHERE f.user_id IS NOT NULL
            OR (r.state = 'requested' AND r.requested_at <= now() - interval '30 days')
            OR r.state = 'completing'
         ORDER BY r.requested_at NULLS LAST, u.id",
    )
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(DueAccount {
                user_id: UserId::new(row.try_get("user_id")?),
                request_id: row.try_get("request_id")?,
                logged: row.try_get("logged")?,
            })
        })
        .collect()
}

/// Claims one due request before any destructive work: the request row is
/// locked first, then the user row, and the claim re-checks
/// `state = 'requested' AND requested_at <= now() - 30 days` before moving the
/// row to `completing` in the same transaction. Returns `false` when the
/// request is no longer owed (it was cancelled meanwhile).
///
/// Two claims survive that re-check on purpose:
/// * `state = 'completing'` — an interrupted run claimed this request already;
///   its work is idempotent and the resume writes no second audit row.
/// * a deletion that is already logged — the request (if a restore put one
///   back) is owed regardless of its clock, and must end up `completed` so it
///   is never due again.
async fn claim_request(
    pool: &PgPool,
    account: &DueAccount,
    _audit: AuditRequest<'_>,
) -> Result<bool, AccountDeletionError> {
    let Some(request_id) = account.request_id else {
        // A logged deletion with no request row: pure replay, nothing to claim.
        return Ok(account.logged);
    };
    let mut tx = pool.begin().await?;
    let _state: Option<String> =
        sqlx::query_scalar("SELECT state FROM account_deletion_requests WHERE id = $1 FOR UPDATE")
            .bind(request_id)
            .fetch_optional(&mut *tx)
            .await?;
    let _user: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE id = $1 FOR UPDATE")
        .bind(account.user_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
    let claimed: Option<Uuid> = sqlx::query_scalar(
        "UPDATE account_deletion_requests
         SET state = 'completing'
         WHERE id = $1
           AND (state = 'completing'
                OR (state = 'requested'
                    AND (requested_at <= now() - interval '30 days' OR $2)))
         RETURNING id",
    )
    .bind(request_id)
    .bind(account.logged)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(claimed.is_some())
}

/// The workspaces the completion considers for one account: every workspace
/// the account is a member of. What happens to each one is decided under the
/// workspace lock in [`complete_workspace`], so a member joining between this
/// selection and the deletion keeps their workspace intact.
pub async fn workspace_candidates(
    pool: &PgPool,
    user: UserId,
) -> Result<Vec<WorkspaceId>, AccountDeletionError> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT workspace_id FROM workspace_memberships
         WHERE user_id = $1
         ORDER BY created_at, id",
    )
    .bind(user.as_uuid())
    .fetch_all(pool)
    .await?;
    Ok(ids.into_iter().map(WorkspaceId::new).collect())
}

/// What [`complete_workspace`] did with one workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceOutcome {
    /// The account solely owned it: its projects, their content and the
    /// workspace itself were deleted in one transaction.
    Deleted {
        /// How many projects (and their content) went with it.
        projects: u64,
    },
    /// A check failed: the workspace is kept and only this account's
    /// membership was removed.
    MembershipRemoved {
        /// The workspace now has no active owner; the operator recovery flow
        /// (03a) picks these up.
        left_ownerless: bool,
    },
    /// The account is no longer a member; nothing to do.
    AlreadyGone,
}

/// The per-workspace step of completion, in ONE transaction: take the same
/// workspace lock 03a's membership writers and workspace deletion use,
/// re-check under it (the account holds an active `owner` membership, no other
/// active membership remains, and the workspace is not the migration
/// workspace), then either delete the projects, their content and the
/// workspace — revoking any pending invitation with audit on the way — or keep
/// the workspace and only remove this account's membership.
///
/// The membership removal deliberately does not apply the voluntary
/// last-owner guard: a shared workspace whose last owner deletes their account
/// is left ownerless for the operator recovery flow, exactly like global
/// deactivation (03a), and the caller reports it.
pub async fn complete_workspace(
    pool: &PgPool,
    actor: UserId,
    workspace: WorkspaceId,
    audit: AuditRequest<'_>,
) -> Result<WorkspaceOutcome, AccountDeletionError> {
    let mut tx = pool.begin().await?;
    match lock_workspace(&mut tx, workspace).await {
        Ok(()) => {}
        Err(WorkspaceError::UnknownWorkspace) => {
            tx.commit().await?;
            return Ok(WorkspaceOutcome::AlreadyGone);
        }
        Err(error) => return Err(error.into()),
    }
    let membership = membership_of(&mut tx, workspace, actor).await?;
    let other_active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workspace_memberships
         WHERE workspace_id = $1 AND user_id <> $2 AND status = 'active'",
    )
    .bind(workspace.as_uuid())
    .bind(actor.as_uuid())
    .fetch_one(&mut *tx)
    .await?;
    let solely_owned = membership.as_ref().is_some_and(|(role, status)| {
        *role == WorkspaceRole::Owner && *status == MembershipStatus::Active
    }) && other_active == 0;
    if solely_owned && workspace != MIGRATION_WORKSPACE_ID {
        let mut projects = 0;
        let project_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM projects WHERE workspace_id = $1 ORDER BY created_at, id",
        )
        .bind(workspace.as_uuid())
        .fetch_all(&mut *tx)
        .await?;
        for project_id in project_ids {
            let project = ProjectId::new(project_id);
            if delete_project_in_transaction(&mut tx, project_id).await? {
                record_project_deleted(&mut tx, Some(actor), workspace, project, audit).await?;
                projects += 1;
            }
        }
        delete_workspace_rows(&mut tx, actor, workspace, audit).await?;
        tx.commit().await?;
        return Ok(WorkspaceOutcome::Deleted { projects });
    }
    if membership.is_some() {
        remove_membership_in_transaction(&mut tx, actor, workspace, actor, audit).await?;
        let other_owners: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM workspace_memberships m
             JOIN users u ON u.id = m.user_id
             WHERE m.workspace_id = $1 AND m.user_id <> $2
               AND m.role = 'owner' AND m.status = 'active' AND u.status = 'active'",
        )
        .bind(workspace.as_uuid())
        .bind(actor.as_uuid())
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(WorkspaceOutcome::MembershipRemoved {
            left_ownerless: other_owners == 0,
        });
    }
    tx.commit().await?;
    Ok(WorkspaceOutcome::AlreadyGone)
}

/// The account's membership in one workspace, if any.
async fn membership_of(
    tx: &mut Transaction<'_, Postgres>,
    workspace: WorkspaceId,
    user: UserId,
) -> Result<Option<(WorkspaceRole, MembershipStatus)>, AccountDeletionError> {
    let row = sqlx::query(
        "SELECT role, status FROM workspace_memberships
         WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace.as_uuid())
    .bind(user.as_uuid())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let role = WorkspaceRole::parse(&row.try_get::<String, _>("role")?)
        .ok_or(WorkspaceError::Corrupt("role"))?;
    let status = MembershipStatus::parse(&row.try_get::<String, _>("status")?)
        .ok_or(WorkspaceError::Corrupt("status"))?;
    Ok(Some((role, status)))
}

/// What [`finalize_account`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FinalizeOutcome {
    /// This run fulfilled the deletion for the first time (audit rows written).
    newly_fulfilled: bool,
    /// This run found something to undo and changed it (a replay).
    changed: bool,
}

/// Erases and deactivates one account in one transaction: the PII goes (the
/// WorkOS link becomes a tombstone), the sessions go, the invitation addresses
/// are scrubbed, the PII in audit metadata is redacted, the request reaches its
/// terminal `completed` state and the fulfillment is logged. Returns whether
/// this run fulfilled the deletion for the first time; only then are the audit
/// rows written.
async fn finalize_account(
    pool: &PgPool,
    account: &DueAccount,
    audit: AuditRequest<'_>,
) -> Result<FinalizeOutcome, AccountDeletionError> {
    let user = account.user_id;
    let mut tx = pool.begin().await?;
    // Lock order: the request row first, then the user row.
    if let Some(request_id) = account.request_id {
        let _request: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM account_deletion_requests WHERE id = $1 FOR UPDATE")
                .bind(request_id)
                .fetch_optional(&mut *tx)
                .await?;
    }
    let row =
        sqlx::query("SELECT id, primary_email, verified_email FROM users WHERE id = $1 FOR UPDATE")
            .bind(user.as_uuid())
            .fetch_optional(&mut *tx)
            .await?;
    let Some(row) = row else {
        return Err(AccountDeletionError::UnknownUser);
    };
    let addresses = {
        let mut addresses = Vec::new();
        addresses.push(row.try_get::<String, _>("primary_email")?);
        if let Some(verified) = row.try_get::<Option<String>, _>("verified_email")? {
            addresses.push(verified);
        }
        addresses
    };
    // A replay uses the first completion's digests, not whichever profile is
    // present after a restore. Normalize and deduplicate before binding the
    // addresses; only their hashes are persisted in the fulfillment log.
    let stored_email_sha256: Option<Vec<String>> = sqlx::query_scalar(
        "SELECT email_sha256 FROM fulfilled_account_deletions WHERE user_id = $1",
    )
    .bind(user.as_uuid())
    .fetch_optional(&mut *tx)
    .await?;
    let (newly_fulfilled, email_sha256) = if let Some(digests) = stored_email_sha256 {
        (false, digests)
    } else {
        let normalized_addresses = normalize_invitation_emails(&addresses);
        let digests: Vec<String> = sqlx::query_scalar(
            "SELECT COALESCE(
               array_agg(DISTINCT encode(digest(email, 'sha256'), 'hex')),
               ARRAY[]::text[])
             FROM unnest($1::text[]) AS normalized(email)",
        )
        .bind(&normalized_addresses)
        .fetch_one(&mut *tx)
        .await?;
        let inserted: Option<Vec<String>> = sqlx::query_scalar(
            "INSERT INTO fulfilled_account_deletions (user_id, email_sha256)
             VALUES ($1, $2)
             ON CONFLICT (user_id) DO NOTHING
             RETURNING email_sha256",
        )
        .bind(user.as_uuid())
        .bind(&digests)
        .fetch_optional(&mut *tx)
        .await?;
        match inserted {
            Some(stored) => (true, stored),
            None => {
                let stored: Vec<String> = sqlx::query_scalar(
                    "SELECT email_sha256 FROM fulfilled_account_deletions WHERE user_id = $1",
                )
                .bind(user.as_uuid())
                .fetch_one(&mut *tx)
                .await?;
                (false, stored)
            }
        }
    };

    // The PII goes; the user id and everything that references it stays.
    // `deleted_at` is the durable marker the trigger of migration 0111 keeps,
    // and the WorkOS tombstone makes a later sign-in a NEW user. The WHERE
    // clause only matches a row that actually differs, so a replay that finds
    // the account already anonymized changes nothing and reports nothing.
    let users_changed = sqlx::query(
        "UPDATE users
         SET status = 'deactivated',
             primary_email = $2,
             display_name = $2,
             verified_email = NULL,
             workos_user_id = $3,
             deleted_at = coalesce(deleted_at, now()),
             updated_at = now()
         WHERE id = $1
           AND (status IS DISTINCT FROM 'deactivated'
                OR primary_email IS DISTINCT FROM $2
                OR display_name IS DISTINCT FROM $2
                OR verified_email IS NOT NULL
                OR workos_user_id IS DISTINCT FROM $3
                OR deleted_at IS NULL)",
    )
    .bind(user.as_uuid())
    .bind(FORMER_MEMBER_LABEL)
    .bind(workos_tombstone(user))
    .execute(&mut *tx)
    .await?
    .rows_affected();

    // Sessions hold the sealed provider payload and go entirely. The live ones
    // leave an audit row behind (the keyed `session_ref` hash, never the
    // credential).
    let session_refs: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT session_ref FROM user_sessions
         WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user.as_uuid())
    .fetch_all(&mut *tx)
    .await?;
    let sessions_deleted = sqlx::query("DELETE FROM user_sessions WHERE user_id = $1")
        .bind(user.as_uuid())
        .execute(&mut *tx)
        .await?
        .rows_affected();

    // Invitation addresses: the pending ones are revoked (audited) and
    // deleted, the historical rows keep their history under the tombstone
    // `deleted-<invitation id>@invalid`. Replays match normalized invitation
    // addresses against the durable digests, never the anonymized profile.
    let invitations_scrubbed = scrub_user_invitations(&mut tx, user, &email_sha256, audit).await?;

    // The native 0103 confirmation rows keep their ids, user references, and
    // confirmation facts; only free-text sources are redacted. Audit metadata
    // is redacted by the SECURITY DEFINER gate in 0117, which authorizes only
    // deleted users or requests already in `completing`.
    let legacy_mappings_scrubbed = sqlx::query(
        "UPDATE legacy_actor_mappings SET source = $2
         WHERE user_id = $1 AND source IS DISTINCT FROM $2",
    )
    .bind(user.as_uuid())
    .bind(REDACTED)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let legacy_non_identities_scrubbed = sqlx::query(
        "UPDATE legacy_actor_non_identities SET source = $2
         WHERE user_id = $1 AND source IS DISTINCT FROM $2",
    )
    .bind(user.as_uuid())
    .bind(REDACTED)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let audit_metadata_scrubbed: i64 = sqlx::query_scalar("SELECT redact_deleted_user_audit($1)")
        .bind(user.as_uuid())
        .fetch_one(&mut *tx)
        .await?;

    // The request reaches its terminal state in the same transaction as the
    // fulfillment, so it is never due again.
    let mut request_completed = false;
    if let Some(request_id) = account.request_id {
        request_completed = sqlx::query(
            "UPDATE account_deletion_requests
             SET state = 'completed', resolved_at = now()
             WHERE id = $1 AND state = 'completing'",
        )
        .bind(request_id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            > 0;
    }

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
    Ok(FinalizeOutcome {
        newly_fulfilled,
        changed: users_changed > 0
            || sessions_deleted > 0
            || invitations_scrubbed > 0
            || legacy_mappings_scrubbed > 0
            || legacy_non_identities_scrubbed > 0
            || audit_metadata_scrubbed > 0
            || request_completed,
    })
}
