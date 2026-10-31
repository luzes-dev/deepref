pub mod collector;
mod generation;
pub mod zip;

use crate::object_lifecycle::{self, ObjectOwner};
use anyhow::{Context, ensure};
use deepref_application::jobs::JobLease;
use sqlx::{Postgres, Transaction};

use chrono::{DateTime, Duration, Utc};
use collector::Source;
use deepref_application::jobs::JobScope;
use deepref_documents::{DocumentStore, ObjectKey};
use deepref_domain::{JobKind, UserId};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub const EXPORT_RETENTION_DAYS: i64 = 7;
pub const MAX_ARCHIVE_BYTES: u64 =
    collector::MAX_SOURCE_BYTES + collector::MAX_JSON_BYTES as u64 + 4 * 1024 * 1024;
pub const EXPORT_ATTESTATION_TOKEN: &str = "all-eager-deleting-binaries-drained-no-restart";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportStatus {
    Requested,
    Processing,
    Completed,
    Failed,
    Cancelled,
    Expired,
}

impl ExportStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Processing => "processing",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "requested" => Some(Self::Requested),
            "processing" => Some(Self::Processing),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountExportRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub status: ExportStatus,
    pub requested_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub retained_until: Option<DateTime<Utc>>,
    pub storage_key: Option<String>,
    pub size_bytes: Option<i64>,
    pub sha256: Option<String>,
    pub failure_reason: Option<String>,
    pub scope_manifest: Option<serde_json::Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("account export rollout gate not activated: requires operator attestation")]
    RetentionNotActivated,

    #[error("account is not active")]
    AccountNotActive,

    #[error("an export request is already active or in progress")]
    ActiveExportExists,

    #[error("export not found")]
    NotFound,

    #[error("export is not in completed state")]
    NotCompleted,

    #[error("export has expired")]
    Expired,

    #[error("export was cancelled")]
    Cancelled,

    #[error("database error")]
    Database(#[from] sqlx::Error),

    #[error("internal error")]
    Other(#[from] anyhow::Error),
}

impl ExportError {
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::RetentionNotActivated => "export_retention_not_activated",
            Self::AccountNotActive => "account_not_active",
            Self::ActiveExportExists => "active_export_exists",
            Self::NotFound => "export_not_found",
            Self::NotCompleted => "export_not_completed",
            Self::Expired => "export_expired",
            Self::Cancelled => "export_cancelled",
            Self::Database(_) => "database_error",
            Self::Other(_) => "internal_error",
        }
    }
}

pub async fn is_export_retention_activated(pool: &PgPool) -> Result<bool, sqlx::Error> {
    let row = sqlx::query_scalar::<_, bool>(
        "SELECT retention_activated FROM account_export_rollout WHERE singleton = true",
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.unwrap_or(false))
}

pub async fn activate_export_retention(pool: &PgPool, attestation: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        attestation == EXPORT_ATTESTATION_TOKEN,
        "invalid attestation token for export retention rollout"
    );

    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE account_export_rollout
         SET retention_activated = true, activated_at = now(), operator_attestation = $1
         WHERE singleton = true",
    )
    .bind(EXPORT_ATTESTATION_TOKEN)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    tracing::info!(
        "Account export retention rollout successfully activated by operator attestation"
    );
    Ok(())
}

pub async fn request_account_export(
    pool: &PgPool,
    user_id: UserId,
    audit: crate::identity::AuditRequest<'_>,
) -> Result<AccountExportRecord, ExportError> {
    if !is_export_retention_activated(pool).await? {
        return Err(ExportError::RetentionNotActivated);
    }

    let mut tx = pool.begin().await?;

    let user_status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM users WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(user_id.as_uuid())
    .fetch_optional(&mut *tx)
    .await?;

    match user_status.as_deref() {
        Some("active") => {}
        _ => return Err(ExportError::AccountNotActive),
    }

    let active_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM account_data_exports
             WHERE user_id = $1 AND status IN ('requested', 'processing')
         )",
    )
    .bind(user_id.as_uuid())
    .fetch_one(&mut *tx)
    .await?;

    if active_exists {
        return Err(ExportError::ActiveExportExists);
    }

    let export_id = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let row = sqlx::query(
        "INSERT INTO account_data_exports (id, user_id, status, job_id)
         VALUES ($1, $2, 'requested', $3)
         RETURNING id, user_id, status, requested_at, completed_at, cancelled_at, retained_until AS expires_at, retained_until, storage_key, size_bytes, sha256, error_message AS failure_reason, scope_manifest",
    )
    .bind(export_id)
    .bind(user_id.as_uuid())
    .bind(job_id)
    .fetch_one(&mut *tx)
    .await?;

    let record = parse_export_row(&row)?;

    let enqueue_job = crate::scoped_job(
        job_id,
        JobScope::Platform,
        JobKind::AccountDataExport,
        serde_json::json!({
            "export_id": export_id,
        }),
        format!("account_export:{export_id}"),
    );
    crate::jobs::enqueue_job(&mut tx, &enqueue_job)
        .await
        .map_err(|e| ExportError::Other(anyhow::anyhow!("failed to enqueue export job: {e}")))?;

    crate::identity::insert_admin_audit(
        &mut tx,
        "account.export_requested",
        crate::identity::AuditPrincipal::User(user_id),
        Some(user_id),
        None,
        None,
        serde_json::json!({ "export_id": export_id }),
        audit,
    )
    .await
    .map_err(|e| ExportError::Other(anyhow::anyhow!("failed to record audit event: {e}")))?;

    tx.commit().await?;

    Ok(record)
}

pub async fn get_latest_account_export(
    pool: &PgPool,
    user_id: UserId,
) -> Result<Option<AccountExportRecord>, ExportError> {
    let row = sqlx::query(
        "SELECT id, user_id, status, requested_at, completed_at, cancelled_at, retained_until AS expires_at, retained_until, storage_key, size_bytes, sha256, error_message AS failure_reason, scope_manifest
         FROM account_data_exports
         WHERE user_id = $1
         ORDER BY requested_at DESC
         LIMIT 1",
    )
    .bind(user_id.as_uuid())
    .fetch_optional(pool)
    .await?;

    row.map(|r| parse_export_row(&r)).transpose()
}

pub async fn get_account_export_by_id(
    pool: &PgPool,
    user_id: UserId,
    export_id: Uuid,
) -> Result<Option<AccountExportRecord>, ExportError> {
    let row = sqlx::query(
        "SELECT id, user_id, status, requested_at, completed_at, cancelled_at, retained_until AS expires_at, retained_until, storage_key, size_bytes, sha256, error_message AS failure_reason, scope_manifest
         FROM account_data_exports
         WHERE id = $1 AND user_id = $2",
    )
    .bind(export_id)
    .bind(user_id.as_uuid())
    .fetch_optional(pool)
    .await?;

    row.map(|r| parse_export_row(&r)).transpose()
}

pub async fn cancel_account_export(
    pool: &PgPool,
    user_id: UserId,
    export_id: Uuid,
    audit: crate::identity::AuditRequest<'_>,
) -> Result<(), ExportError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR SHARE")
        .bind(user_id.as_uuid())
        .execute(&mut *tx)
        .await?;

    let row = sqlx::query(
        "SELECT status FROM account_data_exports
         WHERE id = $1 AND user_id = $2
         FOR UPDATE",
    )
    .bind(export_id)
    .bind(user_id.as_uuid())
    .fetch_optional(&mut *tx)
    .await?;

    let Some(row) = row else {
        return Err(ExportError::NotFound);
    };

    let status_str: String = row.try_get("status")?;
    let status = ExportStatus::parse(&status_str).ok_or(ExportError::NotFound)?;

    if matches!(
        status,
        ExportStatus::Cancelled | ExportStatus::Failed | ExportStatus::Expired
    ) {
        return Ok(());
    }

    sqlx::query(
        "UPDATE account_data_exports
         SET status = 'cancelled', cancelled_at = now(), claim_owner = NULL, claim_seq = NULL
         WHERE id = $1",
    )
    .bind(export_id)
    .execute(&mut *tx)
    .await?;

    release_export_references(&mut tx, export_id)
        .await
        .map_err(|e| {
            ExportError::Other(anyhow::anyhow!(
                "failed to release retention reference: {e}"
            ))
        })?;

    crate::identity::insert_admin_audit(
        &mut tx,
        "account.export_cancelled",
        crate::identity::AuditPrincipal::User(user_id),
        Some(user_id),
        None,
        None,
        serde_json::json!({ "export_id": export_id }),
        audit,
    )
    .await
    .map_err(|e| ExportError::Other(anyhow::anyhow!("failed to record audit event: {e}")))?;

    tx.commit().await?;

    Ok(())
}

fn parse_export_row(row: &PgRow) -> Result<AccountExportRecord, ExportError> {
    let id: Uuid = row.try_get("id")?;
    let user_id: Uuid = row.try_get("user_id")?;
    let status_str: String = row.try_get("status")?;
    let status = ExportStatus::parse(&status_str)
        .ok_or_else(|| ExportError::Other(anyhow::anyhow!("unknown persisted export state")))?;
    let requested_at: DateTime<Utc> = row.try_get("requested_at")?;
    let completed_at: Option<DateTime<Utc>> = row.try_get("completed_at")?;
    let cancelled_at: Option<DateTime<Utc>> = row.try_get("cancelled_at")?;
    let expires_at: Option<DateTime<Utc>> = row.try_get("expires_at")?;
    let retained_until: Option<DateTime<Utc>> = row.try_get("retained_until")?;
    let storage_key: Option<String> = row.try_get("storage_key")?;
    let size_bytes: Option<i64> = row.try_get("size_bytes")?;
    let sha256: Option<String> = row.try_get("sha256")?;
    let failure_reason: Option<String> = row.try_get("failure_reason")?;

    Ok(AccountExportRecord {
        id,
        user_id,
        status,
        requested_at,
        completed_at,
        cancelled_at,
        expires_at,
        retained_until,
        storage_key,
        size_bytes,
        sha256,
        failure_reason,
        scope_manifest: row.try_get("scope_manifest")?,
    })
}

pub async fn record_export_download_audit(
    pool: &PgPool,
    user_id: UserId,
    export_id: Uuid,
    audit: crate::identity::AuditRequest<'_>,
) -> Result<(), ExportError> {
    let mut tx = pool.begin().await?;
    crate::identity::insert_admin_audit(
        &mut tx,
        "account.export_downloaded",
        crate::identity::AuditPrincipal::User(user_id),
        Some(user_id),
        None,
        None,
        serde_json::json!({ "export_id": export_id }),
        audit,
    )
    .await
    .map_err(|e| ExportError::Other(anyhow::anyhow!("failed to record audit event: {e}")))?;
    tx.commit().await?;
    Ok(())
}

/// Locks follow account completion and membership mutation order. The captured
/// UUID set is authoritative: newly acquired workspaces never expand a download.
async fn lock_scope(
    tx: &mut Transaction<'_, Postgres>,
    user: UserId,
    scope: &[Uuid],
) -> anyhow::Result<bool> {
    let active: Option<bool> = sqlx::query_scalar(
        "SELECT status='active' AND deleted_at IS NULL FROM users WHERE id=$1 FOR SHARE",
    )
    .bind(user.as_uuid())
    .fetch_optional(&mut **tx)
    .await?;
    sqlx::query("SELECT id FROM workspaces WHERE id=ANY($1) ORDER BY id FOR SHARE")
        .bind(scope)
        .execute(&mut **tx)
        .await?;
    let owners: Vec<Uuid> = sqlx::query_scalar("SELECT workspace_id FROM workspace_memberships WHERE user_id=$1 AND workspace_id=ANY($2) AND role='owner' AND status='active' ORDER BY workspace_id FOR SHARE")
        .bind(user.as_uuid()).bind(scope).fetch_all(&mut **tx).await?;
    Ok(active == Some(true) && owners == scope)
}

async fn live_claim(tx: &mut Transaction<'_, Postgres>, lease: &JobLease) -> anyhow::Result<()> {
    let valid: Option<bool> = sqlx::query_scalar("SELECT state='running' AND lease_owner=$2 AND claim_seq=$3 AND leased_until>clock_timestamp() AND kind='account_data_export' FROM jobs WHERE id=$1")
        .bind(lease.id).bind(&lease.owner).bind(lease.claim_seq).fetch_optional(&mut **tx).await?;
    ensure!(valid == Some(true), "account export job claim lost");
    Ok(())
}

// Only hold the job row during the short final publication commit. Holding
// it across storage I/O would block the worker's heartbeat lease renewal.
async fn lock_live_claim(
    tx: &mut Transaction<'_, Postgres>,
    lease: &JobLease,
) -> anyhow::Result<()> {
    let id: Option<Uuid> = sqlx::query_scalar("SELECT id FROM jobs WHERE id=$1 AND state='running' AND lease_owner=$2 AND claim_seq=$3 AND leased_until>clock_timestamp() AND kind='account_data_export' FOR SHARE")
        .bind(lease.id).bind(&lease.owner).bind(lease.claim_seq).fetch_optional(&mut **tx).await?;
    ensure!(id.is_some(), "account export job claim lost");
    Ok(())
}

#[derive(Clone)]
struct Attempt {
    export: Uuid,
    user: UserId,
    id: Uuid,
    key: ObjectKey,
}

struct PreparedArchive<'a> {
    inventory: &'a collector::Inventory,
    summary: &'a zip::ArchiveSummary,
    retained_until: DateTime<Utc>,
}

pub async fn process_export_job(
    pool: &PgPool,
    store: &DocumentStore,
    payload: &serde_json::Value,
    lease: &JobLease,
) -> anyhow::Result<()> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Payload {
        export_id: Uuid,
    }
    let payload: Payload = serde_json::from_value(payload.clone())?;
    let Some(attempt) = start_attempt(pool, payload.export_id, lease).await? else {
        return Ok(());
    };
    let work_pool = pool.clone();
    let work_store = store.clone();
    let work_attempt = attempt.clone();
    let work_lease = lease.clone();
    let result = generation::run(
        std::time::Duration::from_secs(600),
        move |budget| async move {
            build_attempt(&work_pool, &work_store, &work_attempt, &work_lease, &budget).await
        },
    )
    .await;
    match result {
        Ok(()) => Ok(()),
        Err(error) => {
            // Persist a closed category, never an upstream response or source
            // value which might contain credentials. A newer attempt is untouched.
            fail_attempt(pool, &attempt, lease, "export_generation_failed").await?;
            Err(error)
        }
    }
}

async fn start_attempt(
    pool: &PgPool,
    export_id: Uuid,
    lease: &JobLease,
) -> anyhow::Result<Option<Attempt>> {
    let owner: Option<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM account_data_exports WHERE id=$1 AND job_id=$2")
            .bind(export_id)
            .bind(lease.id)
            .fetch_optional(pool)
            .await?;
    let user = UserId::new(owner.context("export does not belong to claimed job")?);
    let id = Uuid::new_v4();
    let attempt = Attempt {
        export: export_id,
        user,
        id,
        key: ObjectKey::account_export(user.as_uuid(), id),
    };
    let mut tx = pool.begin().await?;
    let active = lock_scope(&mut tx, user, &[]).await?;
    let status: String =
        sqlx::query_scalar("SELECT status FROM account_data_exports WHERE id=$1 FOR UPDATE")
            .bind(attempt.export)
            .fetch_one(&mut *tx)
            .await?;
    lock_live_claim(&mut tx, lease).await?;
    if !matches!(status.as_str(), "requested" | "processing") {
        return Ok(None);
    }
    release_export_references(&mut tx, attempt.export).await?;
    if !active {
        sqlx::query("UPDATE account_data_exports SET status='failed',failed_at=now(),error_message='account_inactive',claim_owner=NULL,claim_seq=NULL WHERE id=$1")
            .bind(attempt.export).execute(&mut *tx).await?;
        tx.commit().await?;
        return Ok(None);
    }
    sqlx::query("UPDATE account_data_exports SET status='processing',attempt_id=$2,claim_owner=$3,claim_seq=$4,processing_started_at=now(),captured_workspaces='{}' WHERE id=$1")
        .bind(attempt.export).bind(attempt.id).bind(&lease.owner).bind(lease.claim_seq).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Some(attempt))
}

async fn build_attempt(
    pool: &PgPool,
    store: &DocumentStore,
    attempt: &Attempt,
    lease: &JobLease,
    budget: &generation::GenerationBudget,
) -> anyhow::Result<()> {
    budget.check()?;
    let staging = zip::Staging::new()?;
    let (inventory, until) = prepare_attempt(pool, attempt, lease, staging.path(), budget).await?;
    let summary = zip::package(store, &inventory, staging.path(), attempt.user, budget).await?;
    budget.check()?;
    publish_attempt(
        pool,
        store,
        attempt,
        lease,
        PreparedArchive {
            inventory: &inventory,
            summary: &summary,
            retained_until: until,
        },
        budget,
    )
    .await
}

async fn prepare_attempt(
    pool: &PgPool,
    attempt: &Attempt,
    lease: &JobLease,
    staging: &std::path::Path,
    budget: &generation::GenerationBudget,
) -> anyhow::Result<(collector::Inventory, DateTime<Utc>)> {
    // This commits before any long transaction, including with max pool size 1.
    object_lifecycle::register_upload_intent(
        pool,
        ObjectOwner::Account(attempt.user),
        &attempt.key,
    )
    .await?;
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout='60s'")
        .execute(&mut *tx)
        .await?;
    ensure!(
        lock_scope(&mut tx, attempt.user, &[]).await?,
        "account inactive"
    );
    let scope: Vec<Uuid> = sqlx::query_scalar("SELECT workspace_id FROM workspace_memberships WHERE user_id=$1 AND role='owner' AND status='active' ORDER BY workspace_id LIMIT $2")
        .bind(attempt.user.as_uuid()).bind(i64::try_from(collector::MAX_WORKSPACES + 1)?).fetch_all(&mut *tx).await?;
    ensure!(
        scope.len() <= collector::MAX_WORKSPACES,
        "workspace budget exceeded"
    );
    ensure!(
        lock_scope(&mut tx, attempt.user, &scope).await?,
        "ownership lost"
    );
    fence_attempt(&mut tx, attempt, lease).await?;
    let inventory = collector::collect(&mut tx, attempt.user, scope, staging, budget).await?;
    let until = Utc::now() + Duration::days(EXPORT_RETENTION_DAYS);
    let mut sources: Vec<_> = inventory
        .blobs
        .iter()
        .filter_map(|blob| match &blob.source {
            Source::Object { key, project } => Some((key, *project)),
            Source::Inline(_) => None,
        })
        .collect();
    sources.sort_by_key(|(key, _)| key.as_str());
    // Account artifact sorts before all document/workflow keys.
    object_lifecycle::lock_object_for_upload(&mut tx, &attempt.key).await?;
    for (key, project) in sources {
        budget.check()?;
        object_lifecycle::retain_object_reference(
            &mut tx,
            ObjectOwner::Project(project),
            key,
            attempt.id,
            until,
        )
        .await?;
    }
    sqlx::query(
        "UPDATE account_data_exports SET captured_workspaces=$2,retained_until=$3 WHERE id=$1",
    )
    .bind(attempt.export)
    .bind(&inventory.workspaces)
    .bind(until)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((inventory, until))
}

async fn publish_attempt(
    pool: &PgPool,
    store: &DocumentStore,
    attempt: &Attempt,
    lease: &JobLease,
    archive: PreparedArchive<'_>,
    budget: &generation::GenerationBudget,
) -> anyhow::Result<()> {
    budget.check()?;
    let PreparedArchive {
        inventory,
        summary,
        retained_until: until,
    } = archive;
    let mut tx = pool.begin().await?;
    let authorized = lock_scope(&mut tx, attempt.user, &inventory.workspaces).await?;
    fence_attempt(&mut tx, attempt, lease).await?;
    if !authorized {
        terminalize(&mut tx, attempt.export, "failed", Some("ownership_lost")).await?;
        tx.commit().await?;
        return Ok(());
    }
    object_lifecycle::lock_object_for_upload(&mut tx, &attempt.key).await?;
    let file = tokio::fs::File::open(&summary.path).await?;
    let stream = zip::file_stream(file);
    let stored = store
        .for_upload(attempt.key.clone())
        .put_stream(stream)
        .await?;
    budget.check()?;
    ensure!(
        stored.byte_size as u64 == summary.total_bytes && stored.sha256 == summary.sha256_hex,
        "uploaded artifact integrity mismatch"
    );
    // Clock time, not transaction-start now(), fences a lease expiring during I/O.
    lock_live_claim(&mut tx, lease).await?;
    ensure!(
        until > Utc::now(),
        "export retention expired during generation"
    );
    object_lifecycle::retain_object_reference(
        &mut tx,
        ObjectOwner::Account(attempt.user),
        &attempt.key,
        attempt.id,
        until,
    )
    .await?;
    sqlx::query("UPDATE account_data_exports SET status='completed',completed_at=now(),storage_key=$2,size_bytes=$3,sha256=$4,scope_manifest=$5,claim_owner=NULL,claim_seq=NULL WHERE id=$1")
        .bind(attempt.export).bind(attempt.key.as_str()).bind(i64::try_from(summary.total_bytes)?).bind(&summary.sha256_hex).bind(&summary.manifest).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

async fn fence_attempt(
    tx: &mut Transaction<'_, Postgres>,
    attempt: &Attempt,
    lease: &JobLease,
) -> anyhow::Result<()> {
    let valid: bool = sqlx::query_scalar("SELECT status='processing' AND attempt_id=$2 AND claim_owner=$3 AND claim_seq=$4 FROM account_data_exports WHERE id=$1 FOR UPDATE")
        .bind(attempt.export).bind(attempt.id).bind(&lease.owner).bind(lease.claim_seq).fetch_one(&mut **tx).await?;
    ensure!(valid, "export attempt no longer current");
    live_claim(tx, lease).await
}

async fn terminalize(
    tx: &mut Transaction<'_, Postgres>,
    export: Uuid,
    state: &str,
    reason: Option<&str>,
) -> anyhow::Result<()> {
    release_export_references(tx, export).await?;
    sqlx::query("UPDATE account_data_exports SET status=$2,claim_owner=NULL,claim_seq=NULL,failed_at=CASE WHEN $2='failed' THEN now() END,error_message=$3 WHERE id=$1")
        .bind(export).bind(state).bind(reason).execute(&mut **tx).await?;
    Ok(())
}
async fn fail_attempt(
    pool: &PgPool,
    attempt: &Attempt,
    lease: &JobLease,
    reason: &str,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    lock_scope(&mut tx, attempt.user, &[]).await?;
    let valid: bool = sqlx::query_scalar("SELECT status='processing' AND attempt_id=$2 FROM account_data_exports WHERE id=$1 FOR UPDATE")
        .bind(attempt.export).bind(attempt.id).fetch_one(&mut *tx).await?;
    let live: Option<bool> = sqlx::query_scalar("SELECT state='running' AND lease_owner=$2 AND claim_seq=$3 AND leased_until>clock_timestamp() FROM jobs WHERE id=$1 FOR SHARE")
        .bind(lease.id).bind(&lease.owner).bind(lease.claim_seq).fetch_optional(&mut *tx).await?;
    if valid && live == Some(true) {
        terminalize(&mut tx, attempt.export, "failed", Some(reason)).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Bounded maintenance also closes attempts whose durable job exhausted retries.
pub async fn expire_exports(pool: &PgPool) -> anyhow::Result<()> {
    let candidates: Vec<(Uuid,Uuid)> = sqlx::query_as("SELECT e.id,e.user_id FROM account_data_exports e JOIN jobs j ON j.id=e.job_id WHERE (e.status='completed' AND e.retained_until<=now()) OR (e.status IN ('requested','processing') AND j.state IN ('dead','completed','cancelled')) ORDER BY e.id LIMIT 100")
        .fetch_all(pool).await?;
    for (id, user) in candidates {
        let mut tx = pool.begin().await?;
        lock_scope(&mut tx, UserId::new(user), &[]).await?;
        let state: Option<String> = sqlx::query_scalar("SELECT e.status FROM account_data_exports e JOIN jobs j ON j.id=e.job_id WHERE e.id=$1 AND ((e.status='completed' AND e.retained_until<=clock_timestamp()) OR (e.status IN ('requested','processing') AND j.state IN ('dead','completed','cancelled'))) FOR UPDATE OF e")
            .bind(id).fetch_optional(&mut *tx).await?;
        let Some(state) = state else {
            continue;
        };
        if state == "completed" {
            terminalize(&mut tx, id, "expired", None).await?;
        } else if matches!(state.as_str(), "requested" | "processing") {
            terminalize(&mut tx, id, "failed", Some("job_exhausted")).await?;
        }
        tx.commit().await?;
    }
    Ok(())
}

pub async fn authorize_download(
    pool: &PgPool,
    user: UserId,
    id: Uuid,
) -> Result<AccountExportRecord, ExportError> {
    let mut tx = pool.begin().await?;
    let scope: Option<Vec<Uuid>> = sqlx::query_scalar(
        "SELECT captured_workspaces FROM account_data_exports WHERE id=$1 AND user_id=$2",
    )
    .bind(id)
    .bind(user.as_uuid())
    .fetch_optional(&mut *tx)
    .await?;
    let scope = scope.ok_or(ExportError::NotFound)?;
    let authorized = lock_scope(&mut tx, user, &scope).await?;
    let row = sqlx::query("SELECT id,user_id,status,requested_at,completed_at,cancelled_at,retained_until AS expires_at,retained_until,storage_key,size_bytes,sha256,error_message AS failure_reason, scope_manifest,captured_workspaces FROM account_data_exports WHERE id=$1 AND user_id=$2 FOR UPDATE")
        .bind(id).bind(user.as_uuid()).fetch_optional(&mut *tx).await?.ok_or(ExportError::NotFound)?;
    let record = parse_export_row(&row)?;
    if record.status != ExportStatus::Completed {
        return Err(ExportError::NotCompleted);
    }
    let captured: Vec<Uuid> = row.try_get("captured_workspaces")?;
    if !authorized || captured != scope {
        terminalize(&mut tx, id, "failed", Some("ownership_lost")).await?;
        tx.commit().await?;
        return Err(ExportError::NotFound);
    }
    // Reserve enough advertised retention for a bounded five-minute response.
    if record
        .retained_until
        .is_none_or(|until| until <= Utc::now() + Duration::minutes(5))
    {
        terminalize(&mut tx, id, "expired", None).await?;
        tx.commit().await?;
        return Err(ExportError::Expired);
    }
    tx.commit().await?;
    Ok(record)
}

async fn release_export_references(
    tx: &mut Transaction<'_, Postgres>,
    export: Uuid,
) -> anyhow::Result<()> {
    let attempt: Option<Uuid> =
        sqlx::query_scalar("SELECT attempt_id FROM account_data_exports WHERE id=$1")
            .bind(export)
            .fetch_one(&mut **tx)
            .await?;
    if let Some(attempt) = attempt {
        object_lifecycle::release_object_reference(tx, attempt).await?;
    }
    Ok(())
}

/// Caller already holds the account lock. Lock the entire registry union before
/// releasing any export, since multiple attempts can retain overlapping sources.
pub(crate) async fn release_account_exports(
    tx: &mut Transaction<'_, Postgres>,
    user: UserId,
) -> anyhow::Result<()> {
    let exports: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM account_data_exports WHERE user_id=$1 AND status IN ('requested','processing','completed') ORDER BY id FOR UPDATE")
        .bind(user.as_uuid()).fetch_all(&mut **tx).await?;
    sqlx::query("SELECT storage_key FROM object_lifecycle WHERE storage_key IN (SELECT r.storage_key FROM object_retained_references r JOIN account_data_exports e ON e.attempt_id=r.reference_id WHERE e.id=ANY($1)) ORDER BY storage_key FOR UPDATE")
        .bind(&exports).execute(&mut **tx).await?;
    for export in exports {
        release_export_references(tx, export).await?;
        sqlx::query("UPDATE account_data_exports SET status='cancelled',cancelled_at=now(),claim_owner=NULL,claim_seq=NULL WHERE id=$1")
            .bind(export).execute(&mut **tx).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
