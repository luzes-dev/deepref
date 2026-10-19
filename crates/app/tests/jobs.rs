#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use std::time::Duration;

use anyhow::{Result, ensure};
use deepref_app::{PostgresDurableExecutionStore, claim_jobs, job, recover_expired_jobs};
use deepref_application::jobs::{ClaimedJob, DurableExecutionStore, EnqueueJob, JobRetry};
use deepref_domain::{ProjectId, WorkloadClass};

use sqlx::{PgPool, Row};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
}

fn fixture_job(
    project_id: ProjectId,
    id: Uuid,
    kind: deepref_domain::JobKind,
    dedupe_key: String,
) -> EnqueueJob {
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

/// One claim across every workload class: the single-job shape the store's
/// bounded batch claim serves for the minimal contract.
async fn claim_one(
    queue: &PostgresDurableExecutionStore,
    owner: &str,
    lease: Duration,
) -> Result<Option<ClaimedJob>> {
    Ok(DurableExecutionStore::claim(queue, owner, lease, &[], 1)
        .await?
        .pop())
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
    let Some(pool) = database().await else {
        return Ok(());
    };
    let queue = PostgresDurableExecutionStore { pool: pool.clone() };
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
                deepref_domain::JobKind::RecomputeMetrics,
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
                deepref_domain::JobKind::RecomputeMetrics,
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
            deepref_domain::JobKind::RecomputeMetrics,
            claim_dedupe_key,
        );
        // Other workspace integration tests may leave legitimate queued work
        // in the shared database. Keep this concurrency assertion focused on
        // its fixture without changing production queue semantics.
        claim_fixture.priority = i32::MAX;
        queue.enqueue(claim_fixture).await?;
        let (first_claim, second_claim) = tokio::join!(
            claim_one(&queue, "concurrent-owner-a", Duration::from_secs(10)),
            claim_one(&queue, "concurrent-owner-b", Duration::from_secs(10)),
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
    let Some(pool) = database().await else {
        return Ok(());
    };
    let queue = PostgresDurableExecutionStore { pool: pool.clone() };
    let project_id = fixture_project(&pool).await?;
    let lease_job_id = Uuid::new_v4();
    let retry_job_id = Uuid::new_v4();
    let result = async {
        let mut lease_fixture = fixture_job(
            project_id,
            lease_job_id,
            deepref_domain::JobKind::RecomputeMetrics,
            format!("jobs-integration-lease:{lease_job_id}"),
        );
        lease_fixture.priority = i32::MAX;
        queue.enqueue(lease_fixture).await?;
        let stale = claim_one(&queue, "stale-owner", Duration::from_secs(10))
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

        let replacement = claim_one(&queue, "replacement-owner", Duration::from_secs(10))
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
            deepref_domain::JobKind::RecomputeMetrics,
            format!("jobs-integration-retry:{retry_job_id}"),
        );
        retry_job.priority = i32::MAX;
        retry_job.max_attempts = 2;
        queue.enqueue(retry_job).await?;

        let first_attempt = claim_one(&queue, "retry-owner", Duration::from_secs(10))
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
        let second_attempt = claim_one(&queue, "retry-owner", Duration::from_secs(10))
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
    let queue = PostgresDurableExecutionStore { pool: pool.clone() };
    let project_id = fixture_project(&pool).await?;
    let result = async {
        // max_attempts=1: the crashed first attempt exhausts the budget.
        let poison_id = Uuid::new_v4();
        let mut poison = fixture_job(
            project_id,
            poison_id,
            deepref_domain::JobKind::RecomputeMetrics,
            format!("jobs-integration-poison:{poison_id}"),
        );
        poison.max_attempts = 1;
        queue.enqueue(poison).await?;
        let claimed = claim_one(&queue, "crashing-owner", Duration::from_secs(10))
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
            deepref_domain::JobKind::RecomputeMetrics,
            format!("jobs-integration-crash-retry:{retry_id}"),
        );
        retry.max_attempts = 3;
        queue.enqueue(retry).await?;
        claim_one(&queue, "crashing-owner", Duration::from_secs(10))
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
            claim_one(&queue, "next-owner", Duration::from_secs(10))
                .await?
                .is_none(),
            "a backed-off job must not be reclaimed immediately"
        );

        // Second guard: CLAIM never starts an attempt beyond max_attempts.
        let exhausted_id = Uuid::new_v4();
        let mut exhausted = fixture_job(
            project_id,
            exhausted_id,
            deepref_domain::JobKind::RecomputeMetrics,
            format!("jobs-integration-exhausted:{exhausted_id}"),
        );
        exhausted.max_attempts = 2;
        queue.enqueue(exhausted).await?;
        sqlx::query("UPDATE jobs SET attempts=2 WHERE id=$1")
            .bind(exhausted_id)
            .execute(&pool)
            .await?;
        ensure!(
            claim_one(&queue, "next-owner", Duration::from_secs(10))
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
    let queue = PostgresDurableExecutionStore { pool: pool.clone() };
    let project_id = fixture_project(&pool).await?;
    let job_id = Uuid::new_v4();
    let result = async {
        queue
            .enqueue(fixture_job(
                project_id,
                job_id,
                deepref_domain::JobKind::RecomputeMetrics,
                format!("jobs-integration-same-owner:{job_id}"),
            ))
            .await?;
        let first = claim_one(&queue, "one-process", Duration::from_millis(50))
            .await?
            .ok_or_else(|| anyhow::anyhow!("first attempt should claim"))?;
        expire_lease(&pool, job_id).await?;
        ensure!(recover_expired_jobs(&pool).await? == 1);
        make_available(&pool, job_id).await?;
        let second = claim_one(&queue, "one-process", Duration::from_secs(10))
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

#[tokio::test]
async fn an_invalid_persisted_kind_fails_visibly_without_discarding_the_job() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = fixture_project(&pool).await?;
    let job_id = Uuid::new_v4();
    let result = async {
        sqlx::query(
            "INSERT INTO jobs (id,project_id,kind,workload_class,payload,priority,dedupe_key) VALUES ($1,$2,'invalid_persisted_kind','workflow','{}',$3,$4)",
        )
        .bind(job_id)
        .bind(project_id.as_uuid())
        .bind(i32::MAX)
        .bind(format!("invalid-kind:{job_id}"))
        .execute(&pool)
        .await?;
        let queue = PostgresDurableExecutionStore { pool: pool.clone() };
        let error = claim_one(&queue, "invalid-kind-owner", Duration::from_secs(30))
            .await.expect_err("unknown persisted kind must not dispatch or fall back");
        ensure!(format!("{error:#}").contains("invalid_persisted_kind"));
        ensure!(format!("{error:#}").contains(&job_id.to_string()));
        let row = sqlx::query("SELECT kind,state,attempts FROM jobs WHERE id=$1")
            .bind(job_id).fetch_one(&pool).await?;
        ensure!(row.get::<String, _>("kind") == "invalid_persisted_kind");
        ensure!(row.get::<String, _>("state") == "running");
        ensure!(row.get::<i32, _>("attempts") == 1);
        Ok::<_, anyhow::Error>(())
    }.await;
    cleanup(&pool, project_id).await?;
    result
}

/// A rolling deploy runs the previous release's binary (N-1) against this
/// schema. Its enqueue statement knows nothing of `workload_class` or `scope`;
/// the insert must still succeed, the fill trigger classifies the row from its
/// kind, and the N claim loop delivers it in the pool its kind maps to — and
/// in no other pool.
#[tokio::test]
async fn a_previous_release_enqueue_still_classifies_into_the_right_pool() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = fixture_project(&pool).await?;
    let result = async {
        // The N-1 `enqueue_job` verbatim: the exact column list and statement
        // shape the previous release inserts with (git show
        // d3d46a87:crates/app/src/jobs.rs), no workload_class and no scope.
        let mut n1_jobs = Vec::new();
        for (id, kind, expected_class, expected_workload) in [
            (
                Uuid::new_v4(),
                "parse_document",
                "documents",
                WorkloadClass::Documents,
            ),
            (
                Uuid::new_v4(),
                "assistant_agent_run",
                "ai",
                WorkloadClass::Ai,
            ),
        ] {
            let inserted: Uuid = sqlx::query_scalar(
                r#"INSERT INTO jobs (id,project_id,kind,payload,priority,max_attempts,dedupe_key)
                   VALUES ($1,$2,$3,$4,$5,$6,$7)
                   ON CONFLICT (dedupe_key) DO UPDATE SET id = jobs.id
                   RETURNING id"#,
            )
            .bind(id)
            .bind(project_id.as_uuid())
            .bind(kind)
            .bind(serde_json::json!({"fixture_job_id": id}))
            .bind(0i32)
            .bind(5i32)
            .bind(format!("n-1-enqueue:{id}"))
            .fetch_one(&pool)
            .await?;
            ensure!(inserted == id, "the N-1 insert returns its canonical id");
            n1_jobs.push((id, expected_workload));

            // The fill trigger classifies the row from its kind, exactly as
            // the N writer would have at enqueue.
            let row = sqlx::query("SELECT workload_class,scope FROM jobs WHERE id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await?;
            ensure!(row.get::<String, _>("workload_class") == expected_class);
            ensure!(row.get::<String, _>("scope") == "project");
            ensure!(expected_workload.as_str() == expected_class);
        }

        // The N claim loop picks each row up in the pool its kind maps to.
        let documents = claim_jobs(
            &pool,
            "n-1-claim-owner",
            Duration::from_secs(30),
            &[WorkloadClass::Documents],
            10,
        )
        .await?;
        ensure!(
            documents.len() == 1,
            "only the documents job is in the pool"
        );
        ensure!(documents[0].id == n1_jobs[0].0);
        ensure!(documents[0].kind == deepref_domain::JobKind::ParseDocument);
        ensure!(documents[0].scope == deepref_application::jobs::JobScope::Project(project_id));

        let ai = claim_jobs(
            &pool,
            "n-1-claim-owner",
            Duration::from_secs(30),
            &[WorkloadClass::Ai],
            10,
        )
        .await?;
        ensure!(ai.len() == 1, "only the ai job is in the pool");
        ensure!(ai[0].id == n1_jobs[1].0);
        ensure!(ai[0].kind == deepref_domain::JobKind::AssistantAgentRun);
        ensure!(ai[0].scope == deepref_application::jobs::JobScope::Project(project_id));

        // Pool isolation holds for N-1 rows: neither pool sees the other's
        // work once its own claim drained it.
        let remaining = claim_jobs(
            &pool,
            "n-1-claim-owner",
            Duration::from_secs(30),
            &[WorkloadClass::Workflow],
            10,
        )
        .await?;
        ensure!(remaining.is_empty(), "no N-1 row leaks into another pool");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, project_id).await?;
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

/// The 05a state-transition matrix: authoritative state decides, the claim
/// path delivers nothing else. Queued and eligible is claimed and fenced;
/// queued-but-not-yet-due, live-leased, completed and dead rows are never
/// delivered; an expired lease is requeued by `recover_expired_jobs` and only
/// delivered again once its persisted due time passes.
#[tokio::test]
async fn the_claim_path_delivers_only_eligible_authoritative_rows() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let queue = PostgresDurableExecutionStore { pool: pool.clone() };
    let project_id = fixture_project(&pool).await?;
    let due_id = Uuid::new_v4();
    let later_id = Uuid::new_v4();
    let result = async {
        let mut due = fixture_job(
            project_id,
            due_id,
            deepref_domain::JobKind::RecomputeMetrics,
            format!("jobs-matrix-due:{due_id}"),
        );
        due.priority = i32::MAX;
        queue.enqueue(due).await?;
        let mut later = fixture_job(
            project_id,
            later_id,
            deepref_domain::JobKind::RecomputeMetrics,
            format!("jobs-matrix-later:{later_id}"),
        );
        later.priority = i32::MAX;
        queue.enqueue(later).await?;
        // Persist a future due time: `available_at` is authoritative, so this
        // row is not delivered early.
        sqlx::query("UPDATE jobs SET available_at=now()+interval '1 hour' WHERE id=$1")
            .bind(later_id)
            .execute(&pool)
            .await?;

        // queued, eligible -> claim/fence, and exactly one attempt is spent.
        let claimed = claim_one(&queue, "matrix-owner", Duration::from_secs(10))
            .await?
            .ok_or_else(|| anyhow::anyhow!("eligible work must be delivered"))?;
        ensure!(claimed.id == due_id, "the due job is delivered first");
        ensure!(claimed.attempts == 1, "the accepted claim is attempt one");

        // queued, not yet eligible -> no-op; the row and its due time stand.
        ensure!(
            claim_one(&queue, "matrix-owner", Duration::from_secs(10))
                .await?
                .is_none(),
            "no other row is due"
        );
        let untouched = sqlx::query("SELECT state,attempts,available_at FROM jobs WHERE id=$1")
            .bind(later_id)
            .fetch_one(&pool)
            .await?;
        ensure!(untouched.get::<String, _>("state") == "queued");
        ensure!(untouched.get::<i32, _>("attempts") == 0);

        // claimed by a live lease -> no-op for every other claimer.
        ensure!(
            claim_one(&queue, "other-owner", Duration::from_secs(10))
                .await?
                .is_none(),
            "live leases are not stolen"
        );

        // claimed with an expired lease -> recovery requeues with a persisted
        // backoff; the row is delivered again only when due.
        expire_lease(&pool, due_id).await?;
        ensure!(recover_expired_jobs(&pool).await? == 1);
        ensure!(
            claim_one(&queue, "matrix-owner", Duration::from_secs(10))
                .await?
                .is_none(),
            "recovered work waits for its backoff"
        );
        make_available(&pool, due_id).await?;
        let reclaimed = claim_one(&queue, "matrix-owner", Duration::from_secs(10))
            .await?
            .ok_or_else(|| anyhow::anyhow!("recovered work must run again when due"))?;
        ensure!(reclaimed.id == due_id && reclaimed.attempts == 2);

        // completed -> no-op forever.
        ensure!(queue.complete(&reclaimed.lease("matrix-owner")).await?);
        ensure!(
            claim_one(&queue, "matrix-owner", Duration::from_secs(10))
                .await?
                .is_none(),
            "completed work is never delivered again"
        );

        // dead (terminal failure) -> no-op forever.
        let dead_id = Uuid::new_v4();
        let mut dead = fixture_job(
            project_id,
            dead_id,
            deepref_domain::JobKind::RecomputeMetrics,
            format!("jobs-matrix-dead:{dead_id}"),
        );
        dead.priority = i32::MAX;
        dead.max_attempts = 1;
        queue.enqueue(dead).await?;
        let dead_claim = claim_one(&queue, "matrix-owner", Duration::from_secs(10))
            .await?
            .ok_or_else(|| anyhow::anyhow!("the dead fixture claims once"))?;
        ensure!(
            queue
                .fail(
                    &dead_claim.lease("matrix-owner"),
                    "terminal",
                    JobRetry::DeadLetter
                )
                .await?
        );
        ensure!(
            claim_one(&queue, "matrix-owner", Duration::from_secs(10))
                .await?
                .is_none(),
            "dead work is never delivered again"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    let cleanup_result = cleanup(&pool, project_id).await;
    cleanup_result?;
    result
}

/// Transport redelivery never multiplies business retries. Under 05a the
/// claim is the delivery, so attempts increment only on accepted claims: a
/// claim released before execution starts refunds its attempt, duplicate
/// deliveries of a live claim increment nothing, and recovery reuses the
/// attempt the expired claim already spent.
#[tokio::test]
async fn business_attempts_increment_only_on_accepted_claims() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let queue = PostgresDurableExecutionStore { pool: pool.clone() };
    let project_id = fixture_project(&pool).await?;
    let released_id = Uuid::new_v4();
    let duplicate_id = Uuid::new_v4();
    let result = async {
        let mut released = fixture_job(
            project_id,
            released_id,
            deepref_domain::JobKind::RecomputeMetrics,
            format!("jobs-retry-release:{released_id}"),
        );
        released.priority = i32::MAX;
        queue.enqueue(released).await?;

        // Repeated claim/release churn (a worker draining mid-batch over and
        // over) spends no attempt: the job stays claimable at full budget.
        for round in 1..=5 {
            let claim = claim_one(&queue, "release-owner", Duration::from_secs(10))
                .await?
                .ok_or_else(|| anyhow::anyhow!("round {round} must claim"))?;
            ensure!(claim.attempts == 1, "the claim is accepted as attempt one");
            ensure!(queue.release(&claim.lease("release-owner")).await?);
            let row =
                sqlx::query("SELECT state,attempts,lease_owner,available_at FROM jobs WHERE id=$1")
                    .bind(released_id)
                    .fetch_one(&pool)
                    .await?;
            ensure!(row.get::<String, _>("state") == "queued");
            ensure!(
                row.get::<i32, _>("attempts") == 0,
                "released claims refund their attempt"
            );
            ensure!(row.get::<Option<String>, _>("lease_owner").is_none());
            ensure!(
                row.get::<chrono::DateTime<chrono::Utc>, _>("available_at") <= chrono::Utc::now(),
                "released work is immediately claimable"
            );
        }
        let executed = claim_one(&queue, "release-owner", Duration::from_secs(10))
            .await?
            .ok_or_else(|| anyhow::anyhow!("the job still has its full budget"))?;
        ensure!(
            queue.complete(&executed.lease("release-owner")).await?,
            "the job completes at its first real execution"
        );
        let row = sqlx::query("SELECT state,attempts FROM jobs WHERE id=$1")
            .bind(released_id)
            .fetch_one(&pool)
            .await?;
        ensure!(
            row.get::<i32, _>("attempts") == 1,
            "one execution, one attempt"
        );

        // A duplicate delivery of one live claim (another claim attempt while
        // the lease is held) changes no attempt and claims nothing.
        let mut duplicate = fixture_job(
            project_id,
            duplicate_id,
            deepref_domain::JobKind::RecomputeMetrics,
            format!("jobs-retry-duplicate:{duplicate_id}"),
        );
        duplicate.priority = i32::MAX;
        queue.enqueue(duplicate).await?;
        let claim = claim_one(&queue, "duplicate-owner", Duration::from_secs(10))
            .await?
            .ok_or_else(|| anyhow::anyhow!("the duplicate fixture claims once"))?;
        ensure!(claim.attempts == 1);
        ensure!(
            claim_one(&queue, "duplicate-owner", Duration::from_secs(10))
                .await?
                .is_none(),
            "a duplicate delivery is a no-op"
        );
        ensure!(
            queue
                .fail(
                    &claim.lease("duplicate-owner"),
                    "retry once",
                    JobRetry::After(Duration::ZERO)
                )
                .await?,
            "the failure persists its due time"
        );
        let row = sqlx::query("SELECT state,attempts FROM jobs WHERE id=$1")
            .bind(duplicate_id)
            .fetch_one(&pool)
            .await?;
        ensure!(
            row.get::<i32, _>("attempts") == 1,
            "failure spends no extra attempt"
        );

        // Recovery reuses the attempt the expired claim already spent.
        let retry = claim_one(&queue, "duplicate-owner", Duration::from_secs(10))
            .await?
            .ok_or_else(|| anyhow::anyhow!("the retry is due at once"))?;
        ensure!(
            retry.attempts == 2,
            "each accepted claim is exactly one attempt"
        );
        expire_lease(&pool, duplicate_id).await?;
        ensure!(recover_expired_jobs(&pool).await? == 1);
        let row = sqlx::query("SELECT state,attempts FROM jobs WHERE id=$1")
            .bind(duplicate_id)
            .fetch_one(&pool)
            .await?;
        ensure!(row.get::<String, _>("state") == "queued");
        ensure!(
            row.get::<i32, _>("attempts") == 2,
            "recovery never increments attempts"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    let cleanup_result = cleanup(&pool, project_id).await;
    cleanup_result?;
    result
}

/// The batch claim returns at most `limit` rows per statement, in priority
/// order, and leaves the rest queued for a later tick.
#[tokio::test]
async fn the_claim_batch_is_bounded_by_its_limit() -> Result<()> {
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = fixture_project(&pool).await?;
    let mut ids = Vec::new();
    let result = async {
        for (priority, label) in [(5, "five"), (1, "one"), (4, "four"), (0, "zero")] {
            let id = Uuid::new_v4();
            ids.push(id);
            let mut job = fixture_job(
                project_id,
                id,
                deepref_domain::JobKind::RecomputeMetrics,
                format!("jobs-batch:{label}:{id}"),
            );
            job.priority = priority;
            PostgresDurableExecutionStore { pool: pool.clone() }
                .enqueue(job)
                .await?;
        }
        let first = claim_jobs(&pool, "batch-owner", Duration::from_secs(10), &[], 2).await?;
        ensure!(first.len() == 2, "one statement claims at most the limit");
        ensure!(
            first[0].id == ids[0] && first[1].id == ids[2],
            "the batch is claimed in priority order, got {:?}",
            first.iter().map(|job| job.id).collect::<Vec<_>>()
        );
        let second = claim_jobs(&pool, "batch-owner", Duration::from_secs(10), &[], 2).await?;
        ensure!(
            second.len() == 2,
            "the next tick claims the rest within its limit"
        );
        ensure!(
            claim_jobs(&pool, "batch-owner", Duration::from_secs(10), &[], 2)
                .await?
                .is_empty(),
            "nothing is claimed in an unbounded loop"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    let cleanup_result = cleanup(&pool, project_id).await;
    cleanup_result?;
    result
}
