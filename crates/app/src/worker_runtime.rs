use std::time::Duration;

use chrono::{DateTime, Utc};
use deepref_application::operational_health::WorkerTransportEvidence;
use deepref_domain::{JobKind, WorkloadClass};
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkerLeaseRecovery {
    pub expired_doi_leases: u64,
    pub expired_jobs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingAcquisitionWork {
    pub acquisition_id: Uuid,
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
    // Generic jobs are the sole execution lease authority.
    let expired_doi_leases = sqlx::query(
        "UPDATE report_fetch_state SET status='failed',owner_token=NULL,last_error=COALESCE(last_error,'lease expired'),updated_at=now() WHERE status='fetching' AND lease_expires_at < now()",
    )
    .execute(pool)
    .await?
    .rows_affected();
    sqlx::query(
        "UPDATE acquisition_items SET status='queued',last_error=COALESCE(last_error,'lease recovered') WHERE status='fetching' AND NOT EXISTS (SELECT 1 FROM jobs j WHERE j.id=acquisition_items.work_event_id AND j.state='running' AND j.leased_until>now())",
    )
    .execute(pool)
    .await?;

    Ok(WorkerLeaseRecovery {
        expired_doi_leases,
        expired_jobs,
    })
}

/// Finds acquisition work whose durable job is missing. Schema details stay in
/// the PostgreSQL adapter; callers receive only the data required to rebuild
/// the job envelope.
pub async fn find_missing_acquisition_work(
    pool: &PgPool,
    limit: i64,
) -> anyhow::Result<Vec<MissingAcquisitionWork>> {
    let rows = sqlx::query(
        "SELECT i.acquisition_id,i.project_id,i.canonical_doi,i.depth,i.parent_doi,i.work_event_id,COALESCE(g.historical_max_depth,g.max_depth) AS max_depth FROM acquisition_items i JOIN acquisition_runs g ON g.id=i.acquisition_id WHERE i.status IN ('queued','fetching') AND g.status NOT IN ('cancelled','completed','failed') AND NOT EXISTS (SELECT 1 FROM jobs j WHERE j.id=i.work_event_id) ORDER BY i.queued_at LIMIT $1",
    )
    .bind(limit.max(1))
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            let acquisition_id: Uuid = row.get("acquisition_id");
            let canonical_doi: String = row.get("canonical_doi");
            let work_event_id = row.get::<Option<Uuid>, _>("work_event_id").ok_or_else(|| {
                anyhow::anyhow!(
                    "queued acquisition item {acquisition_id}|{canonical_doi} has no work_event_id"
                )
            })?;
            Ok(MissingAcquisitionWork {
                acquisition_id,
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

/// Upsert `owner`'s liveness row (`worker_heartbeats`), and prune rows of
/// workers that stopped without a clean shutdown more than a day ago.
pub async fn record_worker_heartbeat(pool: &PgPool, owner: &str) -> anyhow::Result<()> {
    record_worker_heartbeat_inner(pool, owner, None, None).await
}

/// Refresh worker liveness and the configured capacities of its served pools.
/// Keys are the fixed workload class names; no tenant identifiers are stored.
pub async fn record_worker_heartbeat_with_capacities(
    pool: &PgPool,
    owner: &str,
    pool_capacities: &[(WorkloadClass, usize)],
) -> anyhow::Result<()> {
    record_worker_heartbeat_inner(pool, owner, Some(capacity_json(pool_capacities)), None).await
}

/// Copies actual transport evidence unchanged alongside worker liveness.
pub async fn record_worker_heartbeat_with_transport(
    pool: &PgPool,
    owner: &str,
    pool_capacities: &[(WorkloadClass, usize)],
    evidence: WorkerTransportEvidence,
) -> anyhow::Result<()> {
    record_worker_heartbeat_inner(
        pool,
        owner,
        Some(capacity_json(pool_capacities)),
        Some(evidence),
    )
    .await
}

fn capacity_json(capacities: &[(WorkloadClass, usize)]) -> serde_json::Value {
    let mut values = serde_json::Map::new();
    for (class, capacity) in capacities {
        values.insert(
            class.as_str().to_owned(),
            serde_json::Value::from(i64::try_from(*capacity).unwrap_or(i64::MAX)),
        );
    }
    serde_json::Value::Object(values)
}

async fn record_worker_heartbeat_inner(
    pool: &PgPool,
    owner: &str,
    pool_capacities: Option<serde_json::Value>,
    evidence: Option<WorkerTransportEvidence>,
) -> anyhow::Result<()> {
    let (mode, observation) = match evidence {
        None => (None, None),
        Some(WorkerTransportEvidence::Postgres) => (Some("postgres"), None),
        Some(WorkerTransportEvidence::JetStream(observation)) => (Some("jetstream"), observation),
    };
    let known_job_kinds = JobKind::known_kinds();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO worker_heartbeats (owner,pool_capacities,known_job_kinds,transport_mode,transport_ready,transport_observed_at)
         VALUES ($1,COALESCE($2,'{}'::jsonb),$3,$4,$5,$6)
         ON CONFLICT (owner) DO UPDATE SET seen_at=now(),
             pool_capacities=COALESCE($2,worker_heartbeats.pool_capacities),
             known_job_kinds=EXCLUDED.known_job_kinds,transport_mode=EXCLUDED.transport_mode,
             transport_ready=EXCLUDED.transport_ready,transport_observed_at=EXCLUDED.transport_observed_at",
    )
    .bind(owner).bind(pool_capacities).bind(&known_job_kinds).bind(mode)
    .bind(observation.map(|value|value.ready)).bind(observation.map(|value|value.observed_at))
    .execute(&mut *tx).await?;
    sqlx::query("DELETE FROM worker_heartbeats WHERE seen_at < now() - interval '1 day'")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Remove `owner`'s liveness row after a clean shutdown.
pub async fn remove_worker_heartbeat(pool: &PgPool, owner: &str) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM worker_heartbeats WHERE owner = $1")
        .bind(owner)
        .execute(pool)
        .await?;
    Ok(())
}
