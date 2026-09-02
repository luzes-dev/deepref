use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkerLeaseRecovery {
    pub expired_event_claims: u64,
    pub expired_doi_leases: u64,
    pub expired_jobs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingIngestionWork {
    pub ingestion_id: Uuid,
    pub project_id: Uuid,
    pub canonical_doi: String,
    pub depth: i32,
    pub parent_doi: Option<String>,
    pub max_depth: i32,
    pub work_event_id: Uuid,
}

/// Atomically reserves the next provider permit in PostgreSQL and returns the
/// delay the caller should observe after the transaction commits.
pub async fn reserve_provider_permit(
    pool: &PgPool,
    provider: &str,
    rate_per_second: u32,
) -> anyhow::Result<Duration> {
    let spacing_ms = (1_000_u64 / u64::from(rate_per_second.max(1))).max(1) as i64;
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO provider_rate_state (provider, next_permit_at) VALUES ($1, now()) ON CONFLICT DO NOTHING",
    )
    .bind(provider)
    .execute(&mut *tx)
    .await?;
    let permit_at: DateTime<Utc> = sqlx::query_scalar(
        "SELECT GREATEST(next_permit_at, now()) FROM provider_rate_state WHERE provider = $1 FOR UPDATE",
    )
    .bind(provider)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE provider_rate_state SET next_permit_at = $2 + ($3 * interval '1 millisecond'), updated_at = now() WHERE provider = $1",
    )
    .bind(provider)
    .bind(permit_at)
    .bind(spacing_ms)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok((permit_at - Utc::now()).to_std().unwrap_or_default())
}

/// Recovers worker-owned leases without exposing their backing tables to the
/// worker runtime.
pub async fn recover_expired_worker_state(pool: &PgPool) -> anyhow::Result<WorkerLeaseRecovery> {
    let expired_jobs = crate::jobs::recover_expired_jobs(pool).await?;
    let expired_event_claims = sqlx::query(
        "UPDATE processed_events SET owner_token=NULL,last_error=COALESCE(last_error,'lease expired') WHERE completed_at IS NULL AND owner_token IS NOT NULL AND lease_expires_at < now()",
    )
    .execute(pool)
    .await?
    .rows_affected();
    let expired_doi_leases = sqlx::query(
        "UPDATE doi_fetch_state SET status='failed',owner_token=NULL,last_error=COALESCE(last_error,'lease expired'),updated_at=now() WHERE status='fetching' AND lease_expires_at < now()",
    )
    .execute(pool)
    .await?
    .rows_affected();
    sqlx::query(
        "UPDATE ingestion_items SET status='queued',last_error=COALESCE(last_error,'lease recovered') WHERE status='fetching' AND NOT EXISTS (SELECT 1 FROM doi_fetch_state d WHERE d.canonical_doi=ingestion_items.canonical_doi AND d.status='fetching' AND d.lease_expires_at>now())",
    )
    .execute(pool)
    .await?;

    Ok(WorkerLeaseRecovery {
        expired_event_claims,
        expired_doi_leases,
        expired_jobs,
    })
}

/// Finds ingestion work whose durable job is missing. Schema details stay in
/// the PostgreSQL adapter; callers receive only the data required to rebuild
/// the job envelope.
pub async fn find_missing_ingestion_work(
    pool: &PgPool,
    limit: i64,
) -> anyhow::Result<Vec<MissingIngestionWork>> {
    let rows = sqlx::query(
        "SELECT i.ingestion_id,i.project_id,i.canonical_doi,i.depth,i.parent_doi,i.work_event_id,g.max_depth FROM ingestion_items i JOIN ingestions g ON g.id=i.ingestion_id WHERE i.status='queued' AND NOT EXISTS (SELECT 1 FROM jobs j WHERE j.id=i.work_event_id) ORDER BY i.queued_at LIMIT $1",
    )
    .bind(limit.max(1))
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            let ingestion_id: Uuid = row.get("ingestion_id");
            let canonical_doi: String = row.get("canonical_doi");
            let work_event_id = row
                .get::<Option<Uuid>, _>("work_event_id")
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "queued ingestion item {ingestion_id}|{canonical_doi} has no work_event_id"
                    )
                })?;
            Ok(MissingIngestionWork {
                ingestion_id,
                project_id: row.get("project_id"),
                canonical_doi,
                depth: row.get("depth"),
                parent_doi: row.get("parent_doi"),
                max_depth: row.get("max_depth"),
                work_event_id,
            })
        })
        .collect()
}
