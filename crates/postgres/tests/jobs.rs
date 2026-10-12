#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use std::time::Duration;

use anyhow::{Result, ensure};
use deepref_application::jobs::{EnqueueJob, JobQueue, JobRetry};
use deepref_domain::ProjectId;
use deepref_postgres::{PostgresJobQueue, job, migrate, recover_expired_jobs};
use sqlx::{PgPool, Row};
use uuid::Uuid;

static DATABASE_TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn database() -> Option<PgPool> {
    let pool = deepref_postgres::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await?;
    migrate(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to apply PostgreSQL migrations: {error}"));
    Some(pool)
}

fn fixture_job(project_id: ProjectId, id: Uuid, kind: &str, dedupe_key: String) -> EnqueueJob {
    job(
        id,
        project_id,
        kind,
        serde_json::json!({"fixture_job_id": id}),
        dedupe_key,
    )
}

async fn fixture_project(pool: &PgPool) -> Result<ProjectId> {
    let project_id = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,$2)")
        .bind(project_id.as_uuid())
        .bind(format!("job queue fixture {}", project_id.as_uuid()))
        .execute(pool)
        .await?;
    Ok(project_id)
}

async fn cleanup(pool: &PgPool, project_id: ProjectId) -> Result<()> {
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id.as_uuid())
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
    let project_id = fixture_project(&pool).await?;
    let first_id = Uuid::new_v4();
    let duplicate_id = Uuid::new_v4();
    let claim_id = Uuid::new_v4();
    let dedupe_key = format!("jobs-integration-dedupe:{first_id}");
    let claim_dedupe_key = format!("jobs-integration-claim:{claim_id}");
    let result = async {
        let canonical_id = queue
            .enqueue(fixture_job(
                project_id,
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
                project_id,
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

        let mut claim_fixture = fixture_job(
            project_id,
            claim_id,
            "integration_claim",
            claim_dedupe_key,
        );
        // Other workspace integration tests may leave legitimate queued work
        // in the shared database. Keep this concurrency assertion focused on
        // its fixture without changing production queue semantics.
        claim_fixture.priority = i32::MAX;
        queue.enqueue(claim_fixture).await?;
        let (first_claim, second_claim) = tokio::join!(
            queue.claim("concurrent-owner-a", Duration::from_secs(10)),
            queue.claim("concurrent-owner-b", Duration::from_secs(10)),
        );
        let first_claim = first_claim?;
        let second_claim = second_claim?;
        let fixture_claim_count = [&first_claim, &second_claim]
            .into_iter()
            .filter(|claim| claim.as_ref().is_some_and(|job| job.id == claim_id))
            .count();
        ensure!(
            fixture_claim_count == 1,
            "concurrent owners must claim the fixture exactly once"
        );
        for unrelated in [&first_claim, &second_claim]
            .into_iter()
            .filter_map(Option::as_ref)
            .filter(|job| job.id != claim_id)
        {
            sqlx::query(
                "UPDATE jobs SET state='queued',lease_owner=NULL,leased_until=NULL,lease_renewed_at=NULL WHERE id=$1",
            )
            .bind(unrelated.id)
            .execute(&pool)
            .await?;
        }
        let claimed = first_claim
            .filter(|job| job.id == claim_id)
            .or_else(|| second_claim.filter(|job| job.id == claim_id))
            .ok_or_else(|| anyhow::anyhow!("exclusive claim should exist"))?;
        ensure!(claimed.id == claim_id, "the fixture job should be claimed");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    let cleanup_result = cleanup(&pool, project_id).await;
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
    let project_id = fixture_project(&pool).await?;
    let lease_job_id = Uuid::new_v4();
    let retry_job_id = Uuid::new_v4();
    let result = async {
        let mut lease_fixture = fixture_job(
            project_id,
            lease_job_id,
            "integration_lease",
            format!("jobs-integration-lease:{lease_job_id}"),
        );
        lease_fixture.priority = i32::MAX;
        queue.enqueue(lease_fixture).await?;
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
        expire_lease(&pool, lease_job_id).await?;
        // Recovery backs the job off; skip the wait instead of sleeping.
        ensure!(recover_expired_jobs(&pool).await? == 1);
        make_available(&pool, lease_job_id).await?;

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
        let stale_lease = stale.lease("stale-owner");
        let replacement_lease = replacement.lease("replacement-owner");
        ensure!(
            !queue.renew(&stale_lease, Duration::from_secs(10)).await?,
            "stale owner must not renew a reclaimed lease"
        );
        ensure!(
            !queue.complete(&stale_lease).await?,
            "stale owner must not complete a reclaimed lease"
        );
        ensure!(
            !queue
                .fail(
                    &stale_lease,
                    "stale failure",
                    JobRetry::After(Duration::from_secs(1))
                )
                .await?,
            "stale owner must not fail a reclaimed lease"
        );
        ensure!(
            queue
                .renew(&replacement_lease, Duration::from_secs(10))
                .await?,
            "replacement owner should renew its lease"
        );
        ensure!(
            queue.complete(&replacement_lease).await?,
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
            project_id,
            retry_job_id,
            "integration_retry",
            format!("jobs-integration-retry:{retry_job_id}"),
        );
        retry_job.priority = i32::MAX;
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
                    &first_attempt.lease("retry-owner"),
                    "first retry error",
                    JobRetry::After(Duration::from_secs(30)),
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
                    &second_attempt.lease("retry-owner"),
                    "terminal retry error",
                    JobRetry::After(Duration::from_secs(30)),
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

    let cleanup_result = cleanup(&pool, project_id).await;
    cleanup_result?;
    result
}

/// A job whose worker dies (OOM, segfault, panic outside the handler) never
/// reaches `fail_job`. Lease recovery must spend the same attempt budget as the
/// graceful path, back off before the next attempt, and dead-letter at
/// `max_attempts`; `CLAIM` must refuse an exhausted job as a second guard.
#[tokio::test]
async fn lease_recovery_dead_letters_at_max_attempts_and_backs_off() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let queue = PostgresJobQueue { pool: pool.clone() };
    let project_id = fixture_project(&pool).await?;
    let result = async {
        // max_attempts=1: the crashed first attempt exhausts the budget.
        let poison_id = Uuid::new_v4();
        let mut poison = fixture_job(
            project_id,
            poison_id,
            "integration_poison",
            format!("jobs-integration-poison:{poison_id}"),
        );
        poison.max_attempts = 1;
        queue.enqueue(poison).await?;
        let claimed = queue
            .claim("crashing-owner", Duration::from_secs(10))
            .await?
            .ok_or_else(|| anyhow::anyhow!("poison job should be claimable"))?;
        ensure!(claimed.id == poison_id && claimed.attempts == 1);
        expire_lease(&pool, poison_id).await?;
        ensure!(recover_expired_jobs(&pool).await? == 1);
        let dead =
            sqlx::query("SELECT state,attempts,last_error,lease_owner FROM jobs WHERE id=$1")
                .bind(poison_id)
                .fetch_one(&pool)
                .await?;
        ensure!(
            dead.get::<String, _>("state") == "dead",
            "an expired lease at max_attempts must dead-letter, got {}",
            dead.get::<String, _>("state")
        );
        ensure!(dead.get::<i32, _>("attempts") == 1);
        ensure!(dead.get::<Option<String>, _>("last_error").as_deref() == Some("lease expired"));
        ensure!(dead.get::<Option<String>, _>("lease_owner").is_none());

        // Budget left: requeue with a backoff, so the crash is not retried at once.
        let retry_id = Uuid::new_v4();
        let mut retry = fixture_job(
            project_id,
            retry_id,
            "integration_crash_retry",
            format!("jobs-integration-crash-retry:{retry_id}"),
        );
        retry.max_attempts = 3;
        queue.enqueue(retry).await?;
        queue
            .claim("crashing-owner", Duration::from_secs(10))
            .await?
            .ok_or_else(|| anyhow::anyhow!("retry job should be claimable"))?;
        expire_lease(&pool, retry_id).await?;
        ensure!(recover_expired_jobs(&pool).await? == 1);
        let queued = sqlx::query("SELECT state,last_error,available_at FROM jobs WHERE id=$1")
            .bind(retry_id)
            .fetch_one(&pool)
            .await?;
        ensure!(queued.get::<String, _>("state") == "queued");
        ensure!(queued.get::<Option<String>, _>("last_error").as_deref() == Some("lease expired"));
        ensure!(
            queued.get::<chrono::DateTime<chrono::Utc>, _>("available_at") > chrono::Utc::now(),
            "a recovered lease must back off before the next attempt"
        );
        ensure!(
            queue
                .claim("next-owner", Duration::from_secs(10))
                .await?
                .is_none(),
            "a backed-off job must not be reclaimed immediately"
        );

        // Second guard: CLAIM never starts an attempt beyond max_attempts.
        let exhausted_id = Uuid::new_v4();
        let mut exhausted = fixture_job(
            project_id,
            exhausted_id,
            "integration_exhausted",
            format!("jobs-integration-exhausted:{exhausted_id}"),
        );
        exhausted.max_attempts = 2;
        queue.enqueue(exhausted).await?;
        sqlx::query("UPDATE jobs SET attempts=2 WHERE id=$1")
            .bind(exhausted_id)
            .execute(&pool)
            .await?;
        ensure!(
            queue
                .claim("next-owner", Duration::from_secs(10))
                .await?
                .is_none(),
            "CLAIM must not start attempt max_attempts+1"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    let cleanup_result = cleanup(&pool, project_id).await;
    cleanup_result?;
    result
}

/// The owner string names a worker process, not a claim: a process can reclaim
/// its own expired job. The first attempt's token must then be fenced out, so
/// a slow stale attempt cannot complete, fail or extend the live attempt.
#[tokio::test]
async fn a_stale_attempt_of_the_same_owner_cannot_touch_the_reclaimed_job() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let queue = PostgresJobQueue { pool: pool.clone() };
    let project_id = fixture_project(&pool).await?;
    let job_id = Uuid::new_v4();
    let result = async {
        queue
            .enqueue(fixture_job(
                project_id,
                job_id,
                "integration_same_owner",
                format!("jobs-integration-same-owner:{job_id}"),
            ))
            .await?;
        let first = queue
            .claim("one-process", Duration::from_millis(50))
            .await?
            .ok_or_else(|| anyhow::anyhow!("first attempt should claim"))?;
        expire_lease(&pool, job_id).await?;
        ensure!(recover_expired_jobs(&pool).await? == 1);
        make_available(&pool, job_id).await?;
        let second = queue
            .claim("one-process", Duration::from_secs(10))
            .await?
            .ok_or_else(|| anyhow::anyhow!("the same process should reclaim"))?;
        ensure!(second.id == first.id && second.attempts == first.attempts + 1);

        let stale = first.lease("one-process");
        let live = second.lease("one-process");
        ensure!(
            !queue.renew(&stale, Duration::from_secs(10)).await?,
            "the first attempt must not renew the second attempt's lease"
        );
        ensure!(
            !queue.complete(&stale).await?,
            "the first attempt must not complete the second attempt"
        );
        ensure!(
            !queue
                .fail(&stale, "stale failure", JobRetry::DeadLetter)
                .await?,
            "the first attempt must not fail the second attempt"
        );
        let row = sqlx::query("SELECT state,attempts FROM jobs WHERE id=$1")
            .bind(job_id)
            .fetch_one(&pool)
            .await?;
        ensure!(row.get::<String, _>("state") == "running");
        ensure!(queue.complete(&live).await?, "the live attempt completes");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    let cleanup_result = cleanup(&pool, project_id).await;
    cleanup_result?;
    result
}

/// Force a lease past its expiry boundary instead of sleeping.
async fn expire_lease(pool: &PgPool, job_id: Uuid) -> Result<()> {
    sqlx::query("UPDATE jobs SET leased_until=now()-interval '1 second' WHERE id=$1")
        .bind(job_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Skip a backoff instead of sleeping through it.
async fn make_available(pool: &PgPool, job_id: Uuid) -> Result<()> {
    sqlx::query("UPDATE jobs SET available_at=now()-interval '1 second' WHERE id=$1")
        .bind(job_id)
        .execute(pool)
        .await?;
    Ok(())
}
