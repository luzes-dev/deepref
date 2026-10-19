use std::time::Duration;

use anyhow::Context;
use deepref_application::jobs::{
    ClaimedJob, DurableExecutionStore, EnqueueJob, JobLease, JobRetry, JobScope, WorkRequest,
    WorkTransport,
};
use deepref_domain::{JobKind, ProjectId, WorkloadClass, WorkspaceId, jobs::JobState};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use uuid::Uuid;

/// Claim and fence the next eligible batch for the selected workload pools in
/// one statement. Eligibility is persisted state only: `queued`, due
/// (`available_at <= now()`), and within the attempt budget. Claiming is
/// filtered by workload class, never by `project_id IS NOT NULL`, so workspace-
/// and platform-scoped work is claimable and no row needs a fabricated
/// project. The batch is bounded by `LIMIT $4`; callers must never pass an
/// unbounded limit or loop.
const CLAIM: &str = r#"
WITH candidate AS (
  SELECT id
  FROM jobs
  WHERE state = 'queued' AND available_at <= now()
    AND attempts < max_attempts
    AND (cardinality($3::text[]) = 0 OR workload_class = ANY($3))
  ORDER BY priority DESC, available_at ASC, id ASC
  FOR UPDATE SKIP LOCKED
  LIMIT $4
),
claimed AS (
  UPDATE jobs j
  SET state = 'running', lease_owner = $1,
      leased_until = now() + ($2 * interval '1 millisecond'),
      lease_renewed_at = now(), attempts = j.attempts + 1
  FROM candidate
  WHERE j.id = candidate.id
  RETURNING j.id, j.project_id, j.workspace_id, j.scope, j.kind, j.payload,
            j.attempts, j.max_attempts, j.priority, j.available_at
)
SELECT id, project_id, workspace_id, scope, kind, payload, attempts, max_attempts
FROM claimed
ORDER BY priority DESC, available_at ASC, id ASC
"#;

pub async fn enqueue_job(
    tx: &mut Transaction<'_, Postgres>,
    job: &EnqueueJob,
) -> anyhow::Result<Uuid> {
    let project_id = job.scope.project_id().map(ProjectId::as_uuid);
    let workspace_id = job.scope.workspace_id().map(WorkspaceId::as_uuid);
    let id = sqlx::query_scalar!(
        r#"INSERT INTO jobs (id,project_id,workspace_id,scope,kind,workload_class,payload,priority,max_attempts,dedupe_key)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
           ON CONFLICT (dedupe_key) DO UPDATE SET id = jobs.id
           RETURNING id"#,
        job.id,
        project_id,
        workspace_id,
        job.scope.as_str(),
        job.kind.as_str(),
        job.kind.workload_class().as_str(),
        job.payload,
        job.priority,
        job.max_attempts,
        job.dedupe_key,
    )
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

pub async fn enqueue_job_pool(pool: &PgPool, job: &EnqueueJob) -> anyhow::Result<Uuid> {
    let mut tx = pool.begin().await?;
    let id = enqueue_job(&mut tx, job).await?;
    tx.commit().await?;
    Ok(id)
}

/// Requeue or dead-letter jobs whose lease expired without a graceful
/// `complete_job`/`fail_job` (the worker crashed, was killed, or lost the
/// lease). A lost lease spends an attempt exactly like a reported failure:
/// the job dead-letters at `max_attempts`, and otherwise waits an exponential
/// backoff (30 s doubling per attempt, capped at 15 min) so a job that kills
/// its worker is not reclaimed at once by the next one. Recovery never
/// increments attempts: the expired claim already spent its attempt.
pub async fn recover_expired_jobs(pool: &PgPool) -> anyhow::Result<u64> {
    let recovered = sqlx::query!(
        r#"UPDATE jobs
           SET state = CASE WHEN attempts >= max_attempts THEN 'dead' ELSE 'queued' END,
               last_error = 'lease expired',
               available_at = now()
                 + least(30 * power(2, greatest(attempts - 1, 0)), 900) * interval '1 second',
               lease_owner = NULL,
               leased_until = NULL,
               lease_renewed_at = NULL
           WHERE state = 'running' AND leased_until < now()
           RETURNING id, kind, attempts, max_attempts, state"#,
    )
    .fetch_all(pool)
    .await?;
    for job in &recovered {
        let kind = job
            .kind
            .parse::<JobKind>()
            .with_context(|| format!("invalid persisted kind for job {}", job.id))?;
        let state = job
            .state
            .parse::<JobState>()
            .with_context(|| format!("invalid persisted state for job {}", job.id))?;
        if state == JobState::Dead {
            tracing::error!(
                job_id = %job.id,
                kind = %kind,
                attempts = job.attempts,
                max_attempts = job.max_attempts,
                "job dead-lettered after its lease expired"
            );
        } else {
            tracing::warn!(
                job_id = %job.id,
                kind = %kind,
                attempts = job.attempts,
                max_attempts = job.max_attempts,
                "job lease expired; requeued with backoff"
            );
        }
    }
    crate::durable_runs::settle_terminal_step_jobs(pool).await?;
    Ok(recovered.len() as u64)
}

/// Claim and fence up to `limit` eligible jobs in `pools` (empty = every
/// workload class) in one statement. This is the claim/fence authority: each
/// returned job carries the fencing attempt that `renew`, `complete`, `fail`
/// and `release` require, and it is also the only place business attempts
/// increment.
pub async fn claim_jobs(
    pool: &PgPool,
    owner: &str,
    lease: Duration,
    pools: &[WorkloadClass],
    limit: usize,
) -> anyhow::Result<Vec<ClaimedJob>> {
    let pools: Vec<String> = pools.iter().map(|pool| pool.as_str().to_owned()).collect();
    let rows = sqlx::query(CLAIM)
        .bind(owner)
        .bind(lease.as_millis() as i64)
        .bind(&pools)
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(pool)
        .await?;
    rows.iter().map(row_job).collect()
}

/// Claim one job across every workload class. Reclaim before selecting so
/// callers using the minimal contract get crash recovery even when they do not
/// run the reconciler loop.
pub async fn claim_job(
    pool: &PgPool,
    owner: &str,
    lease: Duration,
) -> anyhow::Result<Option<ClaimedJob>> {
    recover_expired_jobs(pool).await?;
    Ok(claim_jobs(pool, owner, lease, &[], 1).await?.pop())
}

fn row_job(row: &PgRow) -> anyhow::Result<ClaimedJob> {
    let id: Uuid = row.try_get("id")?;
    let kind: String = row.try_get("kind")?;
    let scope: String = row.try_get("scope")?;
    let project_id: Option<Uuid> = row.try_get("project_id")?;
    let workspace_id: Option<Uuid> = row.try_get("workspace_id")?;
    let scope = match scope.as_str() {
        "project" => JobScope::Project(ProjectId::new(
            project_id.with_context(|| format!("project-scoped job {id} has no project"))?,
        )),
        "workspace" => JobScope::Workspace(WorkspaceId::new(
            workspace_id.with_context(|| format!("workspace-scoped job {id} has no workspace"))?,
        )),
        "platform" => JobScope::Platform,
        other => anyhow::bail!("invalid persisted scope {other:?} for job {id}"),
    };
    Ok(ClaimedJob {
        id,
        scope,
        kind: kind
            .parse::<JobKind>()
            .with_context(|| format!("invalid persisted kind for job {id}"))?,
        payload: row.try_get("payload")?,
        attempts: row.try_get("attempts")?,
        max_attempts: row.try_get("max_attempts")?,
    })
}

pub async fn renew_job(
    pool: &PgPool,
    lease: &JobLease,
    duration: Duration,
) -> anyhow::Result<bool> {
    let lease_ms = duration.as_millis() as f64;
    Ok(sqlx::query!(
        r#"UPDATE jobs
           SET leased_until = now() + ($3::double precision * interval '1 millisecond'),
               lease_renewed_at = now()
           WHERE id = $1 AND state = 'running' AND lease_owner = $2 AND attempts = $4
             AND leased_until > now()"#,
        lease.id,
        lease.owner,
        lease_ms,
        lease.attempt,
    )
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

pub async fn complete_job(pool: &PgPool, lease: &JobLease) -> anyhow::Result<bool> {
    Ok(sqlx::query!(
        "UPDATE jobs SET state='completed',completed_at=now(),lease_owner=NULL,leased_until=NULL,lease_renewed_at=NULL WHERE id=$1 AND state='running' AND lease_owner=$2 AND attempts=$3 AND leased_until > now()",
        lease.id,
        lease.owner,
        lease.attempt,
    )
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

/// Record a failed attempt. The job is requeued after the retry delay, or
/// dead-lettered when `retry` says so or the attempt budget is spent. The
/// retry delay is persisted as the job's due time (`available_at`); the claim
/// loop picks the job up when it is due.
pub async fn fail_job(
    pool: &PgPool,
    lease: &JobLease,
    error: &str,
    retry: JobRetry,
) -> anyhow::Result<bool> {
    let (dead_letter, retry_after) = match retry {
        JobRetry::After(delay) => (false, delay),
        JobRetry::DeadLetter => (true, Duration::ZERO),
    };
    let retry_ms = retry_after.as_millis() as f64;
    Ok(sqlx::query!(
        r#"UPDATE jobs
           SET state = CASE WHEN $3 OR attempts >= max_attempts THEN 'dead' ELSE 'queued' END,
               last_error = $4,
               available_at = now() + ($5::double precision * interval '1 millisecond'),
               lease_owner = NULL,
               leased_until = NULL,
               lease_renewed_at = NULL
           WHERE id = $1 AND state = 'running' AND lease_owner = $2 AND attempts = $6
             AND leased_until > now()"#,
        lease.id,
        lease.owner,
        dead_letter,
        error,
        retry_ms,
        lease.attempt,
    )
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

/// Hand an accepted but unstarted claim back to the queue (worker drain
/// mid-batch). The row returns to `queued`, immediately claimable, and the
/// claim's attempt is refunded: no business execution was accepted, so no
/// business attempt may be spent. Only valid for claims whose handler never
/// started; the released token must then be discarded.
pub async fn release_job(pool: &PgPool, lease: &JobLease) -> anyhow::Result<bool> {
    Ok(sqlx::query!(
        r#"UPDATE jobs
           SET state = 'queued',
               attempts = greatest(attempts - 1, 0),
               available_at = now(),
               lease_owner = NULL,
               leased_until = NULL,
               lease_renewed_at = NULL
           WHERE id = $1 AND state = 'running' AND lease_owner = $2 AND attempts = $3
             AND leased_until > now()"#,
        lease.id,
        lease.owner,
        lease.attempt,
    )
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

/// PostgreSQL implementation of the durable execution lifecycle
/// ([`DurableExecutionStore`]).
#[derive(Clone)]
pub struct PostgresDurableExecutionStore {
    pub pool: PgPool,
}

impl DurableExecutionStore for PostgresDurableExecutionStore {
    async fn enqueue(&self, job: EnqueueJob) -> anyhow::Result<Uuid> {
        enqueue_job_pool(&self.pool, &job).await
    }

    async fn claim(
        &self,
        owner: &str,
        lease: Duration,
        pools: &[WorkloadClass],
        limit: usize,
    ) -> anyhow::Result<Vec<ClaimedJob>> {
        claim_jobs(&self.pool, owner, lease, pools, limit).await
    }

    async fn renew(&self, lease: &JobLease, duration: Duration) -> anyhow::Result<bool> {
        renew_job(&self.pool, lease, duration).await
    }

    async fn complete(&self, lease: &JobLease) -> anyhow::Result<bool> {
        complete_job(&self.pool, lease).await
    }

    async fn fail(&self, lease: &JobLease, error: &str, retry: JobRetry) -> anyhow::Result<bool> {
        fail_job(&self.pool, lease, error, retry).await
    }

    async fn release(&self, lease: &JobLease) -> anyhow::Result<bool> {
        release_job(&self.pool, lease).await
    }

    async fn recover_expired(&self) -> anyhow::Result<u64> {
        recover_expired_jobs(&self.pool).await
    }
}

/// The first [`WorkTransport`]: the existing PostgreSQL claim path. Delivery
/// is claiming under this transport — `next_work` claims and fences the work
/// it hands to the worker, so the claim is both wakeup and authority.
#[derive(Clone)]
pub struct PostgresClaimTransport {
    pub store: PostgresDurableExecutionStore,
}

impl WorkTransport for PostgresClaimTransport {
    async fn next_work(&self, request: WorkRequest<'_>) -> anyhow::Result<Vec<ClaimedJob>> {
        self.store
            .claim(
                request.owner,
                request.lease,
                &[request.pool],
                request.max_jobs,
            )
            .await
    }
}

/// Project-scoped enqueue helper (the overwhelmingly common shape).
pub fn job(
    id: Uuid,
    project_id: ProjectId,
    kind: JobKind,
    payload: Value,
    dedupe_key: impl Into<String>,
) -> EnqueueJob {
    scoped_job(id, JobScope::Project(project_id), kind, payload, dedupe_key)
}

/// Enqueue helper for any typed job scope: project, workspace or platform.
pub fn scoped_job(
    id: Uuid,
    scope: JobScope,
    kind: JobKind,
    payload: Value,
    dedupe_key: impl Into<String>,
) -> EnqueueJob {
    EnqueueJob {
        id,
        scope,
        kind,
        payload,
        priority: 0,
        max_attempts: 5,
        dedupe_key: dedupe_key.into(),
    }
}
