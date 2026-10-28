//! Due physical erasure obligations, separate from logical anonymization.
//! Never advances a voluntary request's thirty-day cancellation deadline.
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ErasureReport {
    pub activation_required: bool,
    /// Includes due work blocked before object/writer targeting can commit.
    pub due_requests: i64,
    pub pending_accounts: i64,
    pub oldest_account_due_at: Option<DateTime<Utc>>,
    pub unresolved_uploads: i64,
    pub pending_objects: i64,
    pub overdue_objects: i64,
    pub oldest_due_at: Option<DateTime<Utc>>,
    pub object_retry_attempts: i64,
    pub pending_staging_writers: i64,
    pub overdue_staging_writers: i64,
    /// Older completion logs did not establish physical acknowledgements.
    pub historical_unverified_proofs: i64,
    /// Legacy archive contents cannot be inferred from the latest request.
    pub unjournaled_legacy_objects: i64,
}

pub async fn report(pool: &PgPool) -> Result<ErasureReport, sqlx::Error> {
    let due_requests=sqlx::query_scalar("SELECT count(*) FROM account_deletion_requests WHERE state='completing' OR (state='requested' AND requested_at+interval '30 days'<=clock_timestamp())")
        .fetch_one(pool).await?;
    let (pending_accounts, oldest_account_due_at) =
        sqlx::query_as("SELECT count(*),min(due_at) FROM account_erasure_progress")
            .fetch_one(pool)
            .await?;
    let (pending_objects, overdue_objects, oldest_due_at, object_retry_attempts) = sqlx::query_as(
        "SELECT count(*),count(*) FILTER(WHERE privacy_due_at<clock_timestamp()),min(privacy_due_at),coalesce(sum(cleanup_attempts),0)::bigint FROM object_lifecycle WHERE privacy_due_at IS NOT NULL AND deleted_at IS NULL")
        .fetch_one(pool).await?;
    let (pending_staging_writers,overdue_staging_writers)=sqlx::query_as(
        "SELECT count(*),count(*) FILTER(WHERE privacy_due_at<clock_timestamp()) FROM account_export_writers WHERE privacy_due_at IS NOT NULL AND stopped_at IS NULL")
        .fetch_one(pool).await?;
    let historical_unverified_proofs = sqlx::query_scalar(
        "SELECT count(*) FROM fulfilled_account_deletions WHERE physical_verified_at IS NULL",
    )
    .fetch_one(pool)
    .await?;
    let activated: bool =
        sqlx::query_scalar("SELECT activated FROM account_erasure_rollout WHERE singleton")
            .fetch_one(pool)
            .await?;
    let unresolved_uploads=sqlx::query_scalar("SELECT count(*) FROM account_export_writers WHERE privacy_due_at IS NOT NULL AND upload_outcome IN ('dispatched','unresolved')")
        .fetch_one(pool).await?;
    let unjournaled_legacy_objects=sqlx::query_scalar("SELECT count(*) FROM object_lifecycle o WHERE o.user_id IS NOT NULL AND o.deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM account_export_writers w WHERE w.archive_key=o.storage_key)")
        .fetch_one(pool).await?;
    Ok(ErasureReport {
        due_requests,
        pending_accounts,
        oldest_account_due_at,
        unjournaled_legacy_objects,
        unresolved_uploads,
        activation_required: !activated,
        pending_objects,
        overdue_objects,
        oldest_due_at,
        object_retry_attempts,
        pending_staging_writers,
        overdue_staging_writers,
        historical_unverified_proofs,
    })
}

pub const ERASURE_DRAIN_ATTESTATION: &str =
    "all-legacy-export-writers-stopped-and-untracked-staging-removed-no-restart";

pub async fn activate(pool: &PgPool, attestation: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        attestation == ERASURE_DRAIN_ATTESTATION,
        "exact legacy writer/staging cleanup attestation required"
    );
    sqlx::query("SELECT activate_account_erasure($1)")
        .bind(attestation)
        .execute(pool)
        .await?;
    Ok(())
}

/// First persist the pseudonymous retry material, then deactivate the account.
/// An open request was already claimed under request->user locks by the caller.
/// This record must never be confused with the later fulfillment proof.
pub(crate) async fn begin_account(
    pool: &PgPool,
    user: deepref_domain::UserId,
    digest_key: &str,
) -> Result<(), crate::account_deletion::AccountDeletionError> {
    use crate::account_deletion::AccountDeletionError;
    use sqlx::Row;
    let mut tx = pool.begin().await?;
    let row = sqlx::query("SELECT primary_email,verified_email FROM users WHERE id=$1 FOR UPDATE")
        .bind(user.as_uuid())
        .fetch_one(&mut *tx)
        .await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM account_erasure_progress WHERE user_id=$1)",
    )
    .bind(user.as_uuid())
    .fetch_one(&mut *tx)
    .await?;
    if !exists {
        let logged: Option<(Vec<String>,DateTime<Utc>)> = sqlx::query_as("SELECT email_hmac_sha256,coalesce(logical_scrubbed_at,completed_at) FROM fulfilled_account_deletions WHERE user_id=$1")
            .bind(user.as_uuid()).fetch_optional(&mut *tx).await?;
        let (digests, cutoff) = if let Some((digests, cutoff)) = logged {
            (digests, Some(cutoff))
        } else {
            let mut addresses = vec![row.try_get::<String, _>("primary_email")?];
            if let Some(email) = row.try_get::<Option<String>, _>("verified_email")? {
                addresses.push(email);
            }
            let digests = crate::workspaces::normalize_invitation_emails(&addresses)
                .iter()
                .map(|address| {
                    crate::deletion_digest::hmac_normalized_email(address, digest_key.as_bytes())
                        .map_err(|_| AccountDeletionError::DeletionDigest)
                })
                .collect::<Result<std::collections::BTreeSet<_>, _>>()?
                .into_iter()
                .collect::<Vec<_>>();
            (digests, None)
        };
        sqlx::query("INSERT INTO account_erasure_progress(user_id,due_at,email_hmac_sha256,logical_scrubbed_at) VALUES($1,coalesce((SELECT min(requested_at)+interval '30 days' FROM account_deletion_requests WHERE user_id=$1 AND state IN ('completing','completed')),$3,clock_timestamp()),$2,$3)")
            .bind(user.as_uuid()).bind(digests).bind(cutoff).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE users SET status='deactivated' WHERE id=$1 AND status<>'deactivated'")
        .bind(user.as_uuid())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// A deletion is physically ready only with actual registry and writer-owner
/// acknowledgements. Elapsed retention, failed unlinks and abandoned leases
/// never satisfy either predicate.
pub(crate) async fn physical_ready(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT (SELECT activated FROM account_erasure_rollout WHERE singleton) AND NOT EXISTS(SELECT 1 FROM account_erasure_objects t JOIN object_lifecycle o USING(storage_key) WHERE t.user_id=$1 AND o.deleted_at IS NULL) AND NOT EXISTS(SELECT 1 FROM account_erasure_writers t JOIN account_export_writers w USING(attempt_id) WHERE t.user_id=$1 AND (w.stopped_at IS NULL OR w.upload_outcome IN ('dispatched','unresolved')))")
        .bind(user).fetch_one(&mut **tx).await
}

/// Caller holds this workspace exclusively. Export preparation/publication
/// takes its shared lock BEFORE an export row, so no new retaining inventory
/// can race this cancellation. Do not acquire other account locks here.
pub(crate) async fn release_workspace_exports(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    workspace: Uuid,
    due: DateTime<Utc>,
) -> anyhow::Result<Vec<String>> {
    let exports:Vec<Uuid>=sqlx::query_scalar("SELECT e.id FROM account_data_exports e WHERE $1=ANY(e.captured_workspaces) OR EXISTS(SELECT 1 FROM account_export_writers w WHERE w.export_id=e.id AND $1=ANY(w.captured_workspaces)) ORDER BY e.id FOR UPDATE")
        .bind(workspace).fetch_all(&mut **tx).await?;
    // Scope was committed under this workspace's shared lock before any
    // staging write. Holding it exclusively now prevents a new captured writer.
    let attempts:Vec<Uuid>=sqlx::query_scalar("SELECT attempt_id FROM account_export_writers WHERE $1=ANY(captured_workspaces) UNION SELECT attempt_id FROM account_data_exports WHERE id=ANY($2) AND $1=ANY(captured_workspaces) AND attempt_id IS NOT NULL ORDER BY attempt_id")
        .bind(workspace).bind(&exports).fetch_all(&mut **tx).await?;
    let archives:Vec<String>=sqlx::query_scalar("SELECT archive_key FROM account_export_writers WHERE attempt_id=ANY($1) UNION SELECT 'account-exports/'||user_id::text||'/'||attempt_id::text FROM account_data_exports WHERE id=ANY($2) AND attempt_id=ANY($1) ORDER BY 1")
        .bind(&attempts).bind(&exports).fetch_all(&mut **tx).await?;
    // Registry-before-reference/writer order, including all refs that cancelling
    // the latest request can release. Never take another account's user lock.
    sqlx::query("SELECT storage_key FROM object_lifecycle WHERE workspace_id=$1 OR storage_key=ANY($2) OR storage_key IN (SELECT r.storage_key FROM object_retained_references r WHERE r.reference_id=ANY($3) OR r.reference_id IN(SELECT attempt_id FROM account_data_exports WHERE id=ANY($4))) ORDER BY storage_key FOR UPDATE")
        .bind(workspace).bind(&archives).bind(&attempts).bind(&exports).execute(&mut **tx).await?;
    sqlx::query("UPDATE object_lifecycle SET privacy_due_at=LEAST(privacy_due_at,$2) WHERE workspace_id=$1 OR storage_key=ANY($3)")
        .bind(workspace).bind(due).bind(&archives).execute(&mut **tx).await?;
    sqlx::query("UPDATE account_export_writers SET privacy_due_at=LEAST(privacy_due_at,$2) WHERE attempt_id=ANY($1)")
        .bind(&attempts).bind(due).execute(&mut **tx).await?;
    let keys:Vec<String>=sqlx::query_scalar("SELECT storage_key FROM object_lifecycle WHERE workspace_id=$1 OR storage_key=ANY($2) ORDER BY storage_key")
        .bind(workspace).bind(&archives).fetch_all(&mut **tx).await?;
    sqlx::query("INSERT INTO account_erasure_objects(user_id,storage_key) SELECT $1,unnest($2::text[]) ON CONFLICT DO NOTHING")
        .bind(user).bind(&keys).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO account_erasure_writers(user_id,attempt_id) SELECT $1,attempt_id FROM account_export_writers WHERE attempt_id=ANY($2) ON CONFLICT DO NOTHING")
        .bind(user).bind(&attempts).execute(&mut **tx).await?;
    for attempt in attempts {
        crate::object_lifecycle::release_object_reference(tx, attempt).await?;
    }
    for export in exports {
        crate::account_export::cancel_for_erasure(tx, export).await?;
    }
    Ok(keys)
}

/// Caller holds the account lock and already released its exports.
pub(crate) async fn target_account(
    tx: &mut Transaction<'_, Postgres>,
    user: Uuid,
    due: DateTime<Utc>,
) -> anyhow::Result<()> {
    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT storage_key FROM object_lifecycle WHERE user_id=$1 ORDER BY storage_key FOR UPDATE",
    )
    .bind(user)
    .fetch_all(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE object_lifecycle SET privacy_due_at=LEAST(privacy_due_at,$2) WHERE user_id=$1",
    )
    .bind(user)
    .bind(due)
    .execute(&mut **tx)
    .await?;
    sqlx::query("UPDATE account_export_writers SET privacy_due_at=LEAST(privacy_due_at,$2) WHERE user_id=$1")
        .bind(user).bind(due).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO account_erasure_objects(user_id,storage_key) SELECT $1,unnest($2::text[]) ON CONFLICT DO NOTHING")
        .bind(user).bind(&keys).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO account_erasure_writers(user_id,attempt_id) SELECT $1,attempt_id FROM account_export_writers WHERE user_id=$1 ON CONFLICT DO NOTHING")
        .bind(user).execute(&mut **tx).await?;
    retire_due(tx, &keys).await
}

/// Invoke after metadata and captured exports are removed, in that same
/// transaction. Retirement commits before object-store deletion and never
/// changes an object's immutable identity. Retained/live references fail safe.
pub(crate) async fn retire_due(
    tx: &mut Transaction<'_, Postgres>,
    keys: &[String],
) -> anyhow::Result<()> {
    sqlx::query("UPDATE object_lifecycle o SET retired_at=coalesce(retired_at,clock_timestamp()),cleanup_after=clock_timestamp() WHERE o.storage_key=ANY($1) AND o.privacy_due_at<=clock_timestamp() AND o.deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM documents d WHERE d.object_key=o.storage_key) AND NOT EXISTS(SELECT 1 FROM workflow_files f WHERE f.storage_key=o.storage_key) AND NOT EXISTS(SELECT 1 FROM object_retained_references r WHERE r.storage_key=o.storage_key AND r.expires_at>clock_timestamp())")
        .bind(keys).execute(&mut **tx).await?;
    Ok(())
}
