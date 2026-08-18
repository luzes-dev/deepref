use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct LeasedJob {
    pub id: Uuid,
    pub kind: String,
    pub payload: Value,
    pub attempts: i32,
    pub max_attempts: i32,
    pub leased_until: Option<DateTime<Utc>>,
}

pub async fn recover_expired_leases(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE jobs
        SET state = 'queued', lease_owner = NULL, leased_until = NULL
        WHERE state = 'running'
          AND leased_until IS NOT NULL
          AND leased_until < now()
          AND attempts < max_attempts
        "#,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn lease_jobs(
    pool: &PgPool,
    worker_id: &str,
    limit: i64,
    lease_duration: Duration,
) -> Result<Vec<LeasedJob>, sqlx::Error> {
    let leased_until = Utc::now() + lease_duration;
    sqlx::query_as::<_, LeasedJob>(
        r#"
        WITH candidate AS (
          SELECT id
          FROM jobs
          WHERE state = 'queued'
            AND available_at <= now()
            AND (leased_until IS NULL OR leased_until < now())
          ORDER BY priority DESC, available_at ASC, id ASC
          FOR UPDATE SKIP LOCKED
          LIMIT $1
        )
        UPDATE jobs AS job
        SET state = 'running',
            lease_owner = $2,
            leased_until = $3,
            attempts = attempts + 1
        FROM candidate
        WHERE job.id = candidate.id
        RETURNING job.id, job.kind, job.payload, job.attempts, job.max_attempts, job.leased_until
        "#,
    )
    .bind(limit.max(1))
    .bind(worker_id)
    .bind(leased_until)
    .fetch_all(pool)
    .await
}

pub async fn complete_job(
    transaction: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    worker_id: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE jobs
        SET state = 'completed', completed_at = now(), leased_until = NULL, lease_owner = NULL
        WHERE id = $1 AND state = 'running' AND lease_owner = $2
        "#,
    )
    .bind(job_id)
    .bind(worker_id)
    .execute(&mut **transaction)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn fail_job(
    transaction: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    worker_id: &str,
    error: &str,
    retry_at: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE jobs
        SET state = CASE WHEN attempts >= max_attempts THEN 'dead' ELSE 'queued' END,
            available_at = CASE WHEN attempts >= max_attempts THEN available_at ELSE $4 END,
            last_error = $3,
            leased_until = NULL,
            lease_owner = NULL
        WHERE id = $1 AND state = 'running' AND lease_owner = $2
        "#,
    )
    .bind(job_id)
    .bind(worker_id)
    .bind(error)
    .bind(retry_at)
    .execute(&mut **transaction)
    .await?;
    Ok(result.rows_affected() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leased_job_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LeasedJob>();
    }
}
