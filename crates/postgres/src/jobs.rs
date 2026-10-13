use std::time::Duration;

use deepref_application::{
    AutomationRunId,
    jobs::{ClaimedJob, EnqueueJob, JobLease, JobQueue, JobRetry},
};
use deepref_domain::ProjectId;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

const CLAIM: &str = r#"
WITH candidate AS (
  SELECT id
  FROM jobs
  WHERE state = 'queued' AND project_id IS NOT NULL AND available_at <= now()
    AND attempts < max_attempts
  ORDER BY priority DESC, available_at ASC, id ASC
  FOR UPDATE SKIP LOCKED
  LIMIT 1
)
UPDATE jobs j
SET state = 'running', lease_owner = $1,
    leased_until = now() + ($2 * interval '1 millisecond'),
    lease_renewed_at = now(), attempts = j.attempts + 1
FROM candidate
WHERE j.id = candidate.id
RETURNING j.id, j.project_id, j.kind, j.payload, j.attempts, j.max_attempts
"#;

pub async fn enqueue_job(
    tx: &mut Transaction<'_, Postgres>,
    job: &EnqueueJob,
) -> anyhow::Result<Uuid> {
    let project_id = job.project_id.as_uuid();
    let id = sqlx::query_scalar!(
        r#"INSERT INTO jobs (id,project_id,kind,payload,priority,max_attempts,dedupe_key)
           VALUES ($1,$2,$3,$4,$5,$6,$7)
           ON CONFLICT (dedupe_key) DO UPDATE SET id = jobs.id
           RETURNING id"#,
        job.id,
        project_id,
        job.kind,
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
/// its worker is not reclaimed at once by the next one.
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
        if job.state == "dead" {
            tracing::error!(
                job_id = %job.id,
                kind = %job.kind,
                attempts = job.attempts,
                max_attempts = job.max_attempts,
                "job dead-lettered after its lease expired"
            );
        } else {
            tracing::warn!(
                job_id = %job.id,
                kind = %job.kind,
                attempts = job.attempts,
                max_attempts = job.max_attempts,
                "job lease expired; requeued with backoff"
            );
        }
    }
    Ok(recovered.len() as u64)
}

pub async fn claim_job(
    pool: &PgPool,
    owner: &str,
    lease: Duration,
) -> anyhow::Result<Option<ClaimedJob>> {
    // Reclaim before selecting so callers using the minimal JobQueue contract
    // get crash recovery even when they do not run the reconciler loop.
    recover_expired_jobs(pool).await?;
    let row = sqlx::query(CLAIM)
        .bind(owner)
        .bind(lease.as_millis() as i64)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|row| ClaimedJob {
        id: row.get("id"),
        project_id: ProjectId::new(row.get("project_id")),
        kind: row.get("kind"),
        payload: row.get("payload"),
        attempts: row.get("attempts"),
        max_attempts: row.get("max_attempts"),
    }))
}

/// Resolve the project for an owned automation job without exposing a
/// project-less job row to the worker. The lease predicate is repeated here
/// because the caller may have waited between claiming and dispatching.
pub async fn get_claimed_automation_job_project_id_for_run(
    pool: &PgPool,
    lease: &JobLease,
    run_id: AutomationRunId,
) -> anyhow::Result<Option<ProjectId>> {
    let run_uuid = run_id.as_uuid();
    let project_id = sqlx::query_scalar!(
        r#"SELECT j.project_id as "project_id!"
         FROM jobs AS j
         JOIN automation_runs AS r
           ON r.project_id = j.project_id AND r.job_id = j.id
         WHERE j.id=$1 AND j.kind='automation_run' AND j.state='running'
           AND j.lease_owner=$2 AND j.attempts=$4 AND j.leased_until > now()
           AND r.id=$3 AND j.project_id IS NOT NULL"#,
        lease.id,
        lease.owner,
        run_uuid,
        lease.attempt,
    )
    .fetch_optional(pool)
    .await?;
    Ok(project_id.map(ProjectId::new))
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
/// dead-lettered when `retry` says so or the attempt budget is spent.
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

#[derive(Clone)]
pub struct PostgresJobQueue {
    pub pool: PgPool,
}

impl JobQueue for PostgresJobQueue {
    async fn enqueue(&self, job: EnqueueJob) -> anyhow::Result<Uuid> {
        enqueue_job_pool(&self.pool, &job).await
    }

    async fn claim(&self, owner: &str, lease: Duration) -> anyhow::Result<Option<ClaimedJob>> {
        claim_job(&self.pool, owner, lease).await
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
}

pub fn job(
    id: Uuid,
    project_id: ProjectId,
    kind: impl Into<String>,
    payload: Value,
    dedupe_key: impl Into<String>,
) -> EnqueueJob {
    EnqueueJob {
        id,
        project_id,
        kind: kind.into(),
        payload,
        priority: 0,
        max_attempts: 5,
        dedupe_key: dedupe_key.into(),
    }
}
