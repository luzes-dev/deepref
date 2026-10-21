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

/// Claim the next eligible batch for the selected workload pools in
/// one statement. Eligibility is persisted state only: `queued`, due
/// (`available_at <= now()`), and within the attempt budget. Claiming is
/// filtered by workload class, never by `project_id IS NOT NULL`, so workspace-
/// and platform-scoped work is claimable and no row needs a fabricated
/// project. The batch is bounded by `LIMIT $4`; callers must never pass an
/// unbounded limit or loop. The database advances `claim_seq` for each claim;
/// business attempts remain independently refundable.
///
/// Claims are capability-aware: only the job kinds this binary decodes are
/// ever claimed. A kind a binary cannot decode would fail claim decoding and
/// be dead-lettered, so unknown kinds stay queued for a deployment that
/// understands them instead of being destroyed by this one.
const CLAIM: &str = r#"
WITH candidate AS (
  SELECT id
  FROM jobs
  WHERE state = 'queued' AND available_at <= now()
    AND attempts < max_attempts
    AND kind = ANY($5)
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
            j.claim_seq, j.attempts, j.max_attempts, j.priority, j.available_at
)
SELECT id, project_id, workspace_id, scope, kind, payload, claim_seq, attempts, max_attempts
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

/// Claim up to `limit` eligible jobs in `pools` (empty = every
/// workload class) in one statement. This is the claim/fence authority: each
/// returned job carries a monotonic claim identity for `renew`, `complete`,
/// `fail` and `release`; the business attempt count increments separately.
pub async fn claim_jobs(
    pool: &PgPool,
    owner: &str,
    lease: Duration,
    pools: &[WorkloadClass],
    limit: usize,
) -> anyhow::Result<Vec<ClaimedJob>> {
    let pools: Vec<String> = pools.iter().map(|pool| pool.as_str().to_owned()).collect();
    let kinds = JobKind::known_kinds();
    let rows = sqlx::query(CLAIM)
        .bind(owner)
        .bind(lease.as_millis() as i64)
        .bind(&pools)
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .bind(&kinds)
        .fetch_all(pool)
        .await?;
    let mut claimed = Vec::with_capacity(rows.len());
    for row in &rows {
        match row_job(row) {
            Ok(job) => claimed.push(job),
            Err(error) => {
                let id: Uuid = row.try_get("id")?;
                let claim_seq: i64 = row.try_get("claim_seq")?;
                let reason = truncate_reason(&format!("quarantined after claim decode: {error:#}"));
                match quarantine_claim(pool, id, owner, claim_seq, &reason).await {
                    Ok(true) => tracing::error!(
                        job_id = %id,
                        claim_seq,
                        reason,
                        "malformed claimed job quarantined"
                    ),
                    Ok(false) => tracing::warn!(
                        job_id = %id,
                        claim_seq,
                        "malformed claimed job lost its fence before quarantine"
                    ),
                    Err(quarantine_error) => tracing::error!(
                        job_id = %id,
                        claim_seq,
                        %quarantine_error,
                        "malformed claimed job could not be quarantined; lease recovery will handle it"
                    ),
                }
            }
        }
    }
    Ok(claimed)
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
    let claim_seq: i64 = row.try_get("claim_seq")?;
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
        claim_seq,
        attempts: row.try_get("attempts")?,
        max_attempts: row.try_get("max_attempts")?,
    })
}

fn truncate_reason(reason: &str) -> String {
    reason.chars().take(4096).collect()
}

async fn quarantine_claim(
    pool: &PgPool,
    id: Uuid,
    owner: &str,
    claim_seq: i64,
    reason: &str,
) -> anyhow::Result<bool> {
    Ok(sqlx::query(
        "UPDATE jobs
         SET state='dead', attempts=greatest(attempts - 1, 0), last_error=$4,
             lease_owner=NULL, leased_until=NULL, lease_renewed_at=NULL
         WHERE id=$1 AND state='running' AND lease_owner=$2 AND claim_seq=$3
           AND leased_until > now()",
    )
    .bind(id)
    .bind(owner)
    .bind(claim_seq)
    .bind(reason)
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
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
           WHERE id = $1 AND state = 'running' AND lease_owner = $2 AND claim_seq = $4
             AND leased_until > now()"#,
        lease.id,
        lease.owner,
        lease_ms,
        lease.claim_seq,
    )
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

pub async fn complete_job(pool: &PgPool, lease: &JobLease) -> anyhow::Result<bool> {
    Ok(sqlx::query!(
        "UPDATE jobs SET state='completed',completed_at=now(),lease_owner=NULL,leased_until=NULL,lease_renewed_at=NULL WHERE id=$1 AND state='running' AND lease_owner=$2 AND claim_seq=$3 AND leased_until > now()",
        lease.id,
        lease.owner,
        lease.claim_seq,
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
           WHERE id = $1 AND state = 'running' AND lease_owner = $2 AND claim_seq = $6
             AND leased_until > now()"#,
        lease.id,
        lease.owner,
        dead_letter,
        error,
        retry_ms,
        lease.claim_seq,
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
           WHERE id = $1 AND state = 'running' AND lease_owner = $2 AND claim_seq = $3
             AND leased_until > now()"#,
        lease.id,
        lease.owner,
        lease.claim_seq,
    )
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

async fn release_job_until(
    pool: &PgPool,
    lease: &JobLease,
    deadline: std::time::Instant,
) -> anyhow::Result<bool> {
    const RELEASE_SAFETY_MARGIN: Duration = Duration::from_millis(50);

    let mut transaction = pool.begin().await?;
    let statement_timeout = deadline
        .saturating_duration_since(std::time::Instant::now())
        .saturating_sub(RELEASE_SAFETY_MARGIN);
    let timeout_ms = statement_timeout.as_millis();
    if timeout_ms == 0 {
        return Ok(false);
    }
    let timeout = format!("{timeout_ms}ms");
    sqlx::query(
        "SELECT set_config('statement_timeout', $1, true), set_config('lock_timeout', $1, true)",
    )
    .bind(&timeout)
    .execute(&mut *transaction)
    .await?;

    let result = sqlx::query(
        r#"UPDATE jobs
           SET state = 'queued',
               attempts = greatest(attempts - 1, 0),
               available_at = now(),
               lease_owner = NULL,
               leased_until = NULL,
               lease_renewed_at = NULL
           WHERE id = $1 AND state = 'running' AND lease_owner = $2 AND claim_seq = $3
             AND leased_until > now()"#,
    )
    .bind(lease.id)
    .bind(&lease.owner)
    .bind(lease.claim_seq)
    .execute(&mut *transaction)
    .await?;

    if deadline.saturating_duration_since(std::time::Instant::now()) <= RELEASE_SAFETY_MARGIN {
        return Ok(false);
    }
    transaction.commit().await?;
    Ok(result.rows_affected() == 1)
}

/// Park every queued job of one kind in a state no claim accepts, before a
/// rollback can expose it to a binary that would dead-letter the kind it
/// cannot decode. Runs in the caller's transaction; `reason` names the
/// operator action on the parked rows. Parked rows keep their payload and
/// identity; they are cancelled work awaiting operator review, never silently
/// discarded. Running jobs are untouched: they were already claimed by a
/// binary that understood the kind. Returns the number of parked rows.
pub async fn park_queued_jobs(
    tx: &mut Transaction<'_, Postgres>,
    kind: &JobKind,
    reason: &str,
) -> sqlx::Result<u64> {
    let rows = sqlx::query(
        "UPDATE jobs
         SET state = 'failed', last_error = $2, available_at = now(),
             lease_owner = NULL, leased_until = NULL, lease_renewed_at = NULL
         WHERE kind = $1 AND state = 'queued'",
    )
    .bind(kind.as_str())
    .bind(reason)
    .execute(&mut **tx)
    .await?;
    Ok(rows.rows_affected())
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

    async fn release_until(
        &self,
        lease: &JobLease,
        deadline: std::time::Instant,
    ) -> anyhow::Result<bool> {
        release_job_until(&self.pool, lease, deadline).await
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
