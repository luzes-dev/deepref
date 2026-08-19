use std::time::Duration;

use anyhow::{Result, ensure};
use deepref_application::jobs::{EnqueueJob, JobQueue};
use deepref_postgres::{PostgresJobQueue, job, migrate};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use uuid::Uuid;

static DATABASE_TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .unwrap_or_else(|error| {
            panic!("DATABASE_URL is set but PostgreSQL is unavailable: {error}")
        });
    migrate(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to apply PostgreSQL migrations: {error}"));
    Some(pool)
}

fn fixture_job(id: Uuid, kind: &str, dedupe_key: String) -> EnqueueJob {
    job(
        id,
        kind,
        serde_json::json!({"fixture_job_id": id}),
        dedupe_key,
    )
}

async fn cleanup(pool: &PgPool, ids: &[Uuid]) -> Result<()> {
    sqlx::query("DELETE FROM jobs WHERE id = ANY($1)")
        .bind(ids.to_vec())
        .execute(pool)
        .await?;
    Ok(())
}

#[tokio::test]
async fn enqueue_returns_canonical_id_and_concurrent_claims_are_exclusive() -> Result<()> {
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let queue = PostgresJobQueue { pool: pool.clone() };
    let first_id = Uuid::new_v4();
    let duplicate_id = Uuid::new_v4();
    let claim_id = Uuid::new_v4();
    let dedupe_key = format!("jobs-integration-dedupe:{first_id}");
    let claim_dedupe_key = format!("jobs-integration-claim:{claim_id}");
    let fixture_ids = [first_id, duplicate_id, claim_id];

    let result = async {
        let canonical_id = queue
            .enqueue(fixture_job(
                first_id,
                "integration_dedupe",
                dedupe_key.clone(),
            ))
            .await?;
        ensure!(
            canonical_id == first_id,
            "first enqueue must return its inserted id"
        );

        let duplicate_result = queue
            .enqueue(fixture_job(
                duplicate_id,
                "integration_dedupe",
                dedupe_key.clone(),
            ))
            .await?;
        ensure!(
            duplicate_result == first_id,
            "duplicate enqueue must return the canonical existing row id"
        );

        let count: i64 =
            sqlx::query_scalar("SELECT count(*)::bigint FROM jobs WHERE dedupe_key=$1")
                .bind(&dedupe_key)
                .fetch_one(&pool)
                .await?;
        ensure!(count == 1, "dedupe key must create exactly one job");
        let persisted_id: Uuid = sqlx::query_scalar("SELECT id FROM jobs WHERE dedupe_key=$1")
            .bind(&dedupe_key)
            .fetch_one(&pool)
            .await?;
        ensure!(
            persisted_id == first_id,
            "the persisted dedupe row must be the first job"
        );
        sqlx::query("UPDATE jobs SET state='completed',completed_at=now() WHERE id=$1")
            .bind(first_id)
            .execute(&pool)
            .await?;

        queue
            .enqueue(fixture_job(claim_id, "integration_claim", claim_dedupe_key))
            .await?;
        let (first_claim, second_claim) = tokio::join!(
            queue.claim("concurrent-owner-a", Duration::from_secs(10)),
            queue.claim("concurrent-owner-b", Duration::from_secs(10)),
        );
        let first_claim = first_claim?;
        let second_claim = second_claim?;
        ensure!(
            first_claim.is_some() ^ second_claim.is_some(),
            "concurrent owners must produce exactly one claim for one queued job"
        );
        let claimed = first_claim
            .or(second_claim)
            .ok_or_else(|| anyhow::anyhow!("exclusive claim should exist"))?;
        ensure!(claimed.id == claim_id, "the fixture job should be claimed");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    let cleanup_result = cleanup(&pool, &fixture_ids).await;
    cleanup_result?;
    result
}

#[tokio::test]
async fn expired_leases_are_fenced_and_retries_reach_dead_at_max_attempts() -> Result<()> {
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let queue = PostgresJobQueue { pool: pool.clone() };
    let lease_job_id = Uuid::new_v4();
    let retry_job_id = Uuid::new_v4();
    let fixture_ids = [lease_job_id, retry_job_id];

    let result = async {
        queue
            .enqueue(fixture_job(
                lease_job_id,
                "integration_lease",
                format!("jobs-integration-lease:{lease_job_id}"),
            ))
            .await?;
        let stale = queue
            .claim("stale-owner", Duration::from_secs(10))
            .await?
            .ok_or_else(|| anyhow::anyhow!("stale owner should claim the lease fixture"))?;
        ensure!(
            stale.attempts == 1,
            "the initial claim should be attempt one"
        );

        // Force the lease past its expiry boundary instead of sleeping for a
        // wall-clock interval, keeping the integration test deterministic.
        sqlx::query("UPDATE jobs SET leased_until=now()-interval '1 second' WHERE id=$1")
            .bind(lease_job_id)
            .execute(&pool)
            .await?;

        let replacement = queue
            .claim("replacement-owner", Duration::from_secs(10))
            .await?
            .ok_or_else(|| anyhow::anyhow!("expired lease should be reclaimed"))?;
        ensure!(
            replacement.id == lease_job_id,
            "replacement must reclaim the same job"
        );
        ensure!(
            replacement.attempts == 2,
            "reclaiming a lease must increment attempts"
        );
        ensure!(
            !queue
                .renew("stale-owner", stale.id, Duration::from_secs(10))
                .await?,
            "stale owner must not renew a reclaimed lease"
        );
        ensure!(
            !queue.complete("stale-owner", stale.id).await?,
            "stale owner must not complete a reclaimed lease"
        );
        ensure!(
            !queue
                .fail(
                    "stale-owner",
                    &stale,
                    "stale failure",
                    Duration::from_secs(1)
                )
                .await?,
            "stale owner must not fail a reclaimed lease"
        );
        ensure!(
            queue
                .renew("replacement-owner", replacement.id, Duration::from_secs(10))
                .await?,
            "replacement owner should renew its lease"
        );
        ensure!(
            queue.complete("replacement-owner", replacement.id).await?,
            "replacement owner should complete its lease"
        );

        let completed =
            sqlx::query("SELECT state,attempts,lease_owner,leased_until FROM jobs WHERE id=$1")
                .bind(lease_job_id)
                .fetch_one(&pool)
                .await?;
        ensure!(completed.get::<String, _>("state") == "completed");
        ensure!(completed.get::<i32, _>("attempts") == 2);
        ensure!(completed.get::<Option<String>, _>("lease_owner").is_none());
        ensure!(
            completed
                .get::<Option<chrono::DateTime<chrono::Utc>>, _>("leased_until")
                .is_none()
        );

        let mut retry_job = fixture_job(
            retry_job_id,
            "integration_retry",
            format!("jobs-integration-retry:{retry_job_id}"),
        );
        retry_job.max_attempts = 2;
        queue.enqueue(retry_job).await?;

        let first_attempt = queue
            .claim("retry-owner", Duration::from_secs(10))
            .await?
            .ok_or_else(|| anyhow::anyhow!("retry fixture should be claimed initially"))?;
        ensure!(first_attempt.attempts == 1);
        ensure!(
            queue
                .fail(
                    "retry-owner",
                    &first_attempt,
                    "first retry error",
                    Duration::from_secs(30),
                )
                .await?,
            "first retry failure should be persisted"
        );
        let queued =
            sqlx::query("SELECT state,attempts,last_error,available_at FROM jobs WHERE id=$1")
                .bind(retry_job_id)
                .fetch_one(&pool)
                .await?;
        ensure!(queued.get::<String, _>("state") == "queued");
        ensure!(queued.get::<i32, _>("attempts") == 1);
        ensure!(queued.get::<String, _>("last_error") == "first retry error");
        ensure!(
            queued.get::<chrono::DateTime<chrono::Utc>, _>("available_at") > chrono::Utc::now(),
            "retry failure must persist a future availability time"
        );

        sqlx::query("UPDATE jobs SET available_at=now()-interval '1 second' WHERE id=$1")
            .bind(retry_job_id)
            .execute(&pool)
            .await?;
        let second_attempt = queue
            .claim("retry-owner", Duration::from_secs(10))
            .await?
            .ok_or_else(|| anyhow::anyhow!("queued retry should be claimable"))?;
        ensure!(second_attempt.attempts == 2);
        ensure!(
            queue
                .fail(
                    "retry-owner",
                    &second_attempt,
                    "terminal retry error",
                    Duration::from_secs(30),
                )
                .await?,
            "max-attempt failure should be persisted"
        );
        let dead =
            sqlx::query("SELECT state,attempts,last_error,available_at FROM jobs WHERE id=$1")
                .bind(retry_job_id)
                .fetch_one(&pool)
                .await?;
        ensure!(dead.get::<String, _>("state") == "dead");
        ensure!(
            dead.get::<i32, _>("attempts") == 2,
            "job must reach dead exactly at max_attempts"
        );
        ensure!(dead.get::<String, _>("last_error") == "terminal retry error");
        ensure!(
            dead.get::<chrono::DateTime<chrono::Utc>, _>("available_at") > chrono::Utc::now(),
            "terminal failure must retain its retry availability timestamp"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    let cleanup_result = cleanup(&pool, &fixture_ids).await;
    cleanup_result?;
    result
}
