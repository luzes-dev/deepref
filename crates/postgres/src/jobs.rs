use std::time::Duration;

use deepref_application::{
    AutomationRunId,
    jobs::{ClaimedJob, EnqueueJob, JobQueue},
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
    let id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO jobs (id,project_id,kind,payload,priority,max_attempts,dedupe_key) VALUES ($1,$2,$3,$4,$5,$6,$7) ON CONFLICT (dedupe_key) DO UPDATE SET id = jobs.id RETURNING id",
    )
    .bind(job.id)
    .bind(job.project_id.as_uuid())
    .bind(&job.kind)
    .bind(&job.payload)
    .bind(job.priority)
    .bind(job.max_attempts)
    .bind(&job.dedupe_key)
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

pub async fn recover_expired_jobs(pool: &PgPool) -> anyhow::Result<u64> {
    Ok(sqlx::query(
        "UPDATE jobs SET state='queued',lease_owner=NULL,leased_until=NULL,lease_renewed_at=NULL WHERE state='running' AND leased_until < now()",
    )
    .execute(pool)
    .await?
    .rows_affected())
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
    job_id: Uuid,
    run_id: AutomationRunId,
    owner: &str,
) -> anyhow::Result<Option<ProjectId>> {
    let project_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT j.project_id
         FROM jobs AS j
         JOIN automation_runs AS r
           ON r.project_id = j.project_id AND r.job_id = j.id
         WHERE j.id=$1 AND j.kind='automation_run' AND j.state='running'
           AND j.lease_owner=$2 AND j.leased_until > now()
           AND r.id=$3 AND j.project_id IS NOT NULL",
    )
    .bind(job_id)
    .bind(owner)
    .bind(run_id.as_uuid())
    .fetch_optional(pool)
    .await?;
    Ok(project_id.map(ProjectId::new))
}

pub async fn renew_job(
    pool: &PgPool,
    owner: &str,
    job_id: Uuid,
    lease: Duration,
) -> anyhow::Result<bool> {
    Ok(sqlx::query(
        "UPDATE jobs SET leased_until=now()+($3 * interval '1 millisecond'),lease_renewed_at=now() WHERE id=$1 AND state='running' AND lease_owner=$2 AND leased_until > now()",
    )
    .bind(job_id)
    .bind(owner)
    .bind(lease.as_millis() as i64)
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

pub async fn complete_job(pool: &PgPool, owner: &str, job_id: Uuid) -> anyhow::Result<bool> {
    Ok(sqlx::query(
        "UPDATE jobs SET state='completed',completed_at=now(),lease_owner=NULL,leased_until=NULL,lease_renewed_at=NULL WHERE id=$1 AND state='running' AND lease_owner=$2 AND leased_until > now()",
    )
    .bind(job_id)
    .bind(owner)
    .execute(pool)
    .await?
    .rows_affected()
        == 1)
}

pub async fn fail_job(
    pool: &PgPool,
    owner: &str,
    job: &ClaimedJob,
    error: &str,
    retry_after: Duration,
) -> anyhow::Result<bool> {
    let next_state = if job.attempts >= job.max_attempts {
        "dead"
    } else {
        "queued"
    };
    Ok(sqlx::query(
        "UPDATE jobs SET state=$3,last_error=$4,available_at=now()+($5 * interval '1 millisecond'),lease_owner=NULL,leased_until=NULL,lease_renewed_at=NULL WHERE id=$1 AND state='running' AND lease_owner=$2 AND leased_until > now()",
    )
    .bind(job.id)
    .bind(owner)
    .bind(next_state)
    .bind(error)
    .bind(retry_after.as_millis() as i64)
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

    async fn renew(&self, owner: &str, job_id: Uuid, lease: Duration) -> anyhow::Result<bool> {
        renew_job(&self.pool, owner, job_id, lease).await
    }

    async fn complete(&self, owner: &str, job_id: Uuid) -> anyhow::Result<bool> {
        complete_job(&self.pool, owner, job_id).await
    }

    async fn fail(
        &self,
        owner: &str,
        job: &ClaimedJob,
        error: &str,
        retry_after: Duration,
    ) -> anyhow::Result<bool> {
        fail_job(&self.pool, owner, job, error, retry_after).await
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
