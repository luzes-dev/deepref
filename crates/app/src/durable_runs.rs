//! Canonical lifecycle transitions. The generic queue remains the lease authority.
use deepref_application::jobs::JobLease;
use serde_json::Value;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

pub const DURABLE_RUN_STEP_JOB_KIND: &str = deepref_domain::JobKind::DurableRunStep.as_str();
/// Workload pool of the durable step kind, derived exactly like `enqueue_job`
/// derives it: raw step enqueues never choose a class themselves.
pub const DURABLE_RUN_STEP_WORKLOAD_CLASS: &str = deepref_domain::JobKind::DurableRunStep
    .workload_class()
    .as_str();

/// Lock run before step everywhere, including cancellation and domain planning.
async fn lock_step(
    tx: &mut Transaction<'_, Postgres>,
    step_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let run = sqlx::query_scalar::<_, Uuid>("SELECT run_id FROM durable_steps WHERE id=$1")
        .bind(step_id)
        .fetch_optional(&mut **tx)
        .await?;
    let Some(run) = run else { return Ok(None) };
    let status =
        sqlx::query_scalar::<_, String>("SELECT status FROM durable_runs WHERE id=$1 FOR UPDATE")
            .bind(run)
            .fetch_optional(&mut **tx)
            .await?;
    sqlx::query("SELECT id FROM durable_steps WHERE id=$1 FOR UPDATE")
        .bind(step_id)
        .fetch_optional(&mut **tx)
        .await?;
    Ok(status)
}

async fn live_lease(
    tx: &mut Transaction<'_, Postgres>,
    step_id: Uuid,
    lease: &JobLease,
) -> Result<bool, sqlx::Error> {
    // Holding this lock through the transition prevents lease recovery/claiming
    // from interleaving between the live check and the lifecycle write.
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT j.id FROM jobs j JOIN durable_steps s ON s.project_id=j.project_id
         WHERE s.id=$1 AND j.id=$2 AND j.state='running' AND j.lease_owner=$3
           AND j.claim_seq=$4 AND j.leased_until>clock_timestamp()
           AND j.kind='durable_run_step' AND j.payload->>'run_id'=s.run_id::text
           AND j.payload->>'step_id'=s.id::text
           AND j.payload->>'handler_kind'=s.handler_kind
         FOR UPDATE OF j",
    )
    .bind(step_id)
    .bind(lease.id)
    .bind(&lease.owner)
    .bind(lease.claim_seq)
    .fetch_optional(&mut **tx)
    .await?
    .is_some())
}

/// Fence typed planning and artifact writes without beginning another attempt.
pub async fn verify_active_step_lease(
    tx: &mut Transaction<'_, Postgres>,
    step_id: Uuid,
    lease: &JobLease,
) -> Result<bool, sqlx::Error> {
    let Some(status) = lock_step(tx, step_id).await? else {
        return Ok(false);
    };
    if !matches!(status.as_str(), "queued" | "running" | "waiting") {
        return Ok(false);
    }
    live_lease(tx, step_id, lease).await
}

/// Enrich a completed delegation step from a live claim in the same run.
/// This changes only its business output, never its lifecycle or accepted evidence.
pub async fn set_completed_step_output(
    tx: &mut Transaction<'_, Postgres>,
    step_id: Uuid,
    lease: &JobLease,
    output: &Value,
) -> Result<bool, sqlx::Error> {
    let Some(status) = lock_step(tx, step_id).await? else {
        return Ok(false);
    };
    if !matches!(status.as_str(), "queued" | "running" | "waiting") {
        return Ok(false);
    }
    let current = sqlx::query_scalar::<_, Uuid>(
        "SELECT active.id FROM jobs j JOIN durable_steps active
         ON j.payload->>'step_id'=active.id::text AND j.project_id=active.project_id
         JOIN durable_steps target ON target.id=$1 AND target.run_id=active.run_id
           AND target.project_id=active.project_id WHERE j.id=$2",
    )
    .bind(step_id)
    .bind(lease.id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(current) = current else {
        return Ok(false);
    };
    if !live_lease(tx, current, lease).await? {
        return Ok(false);
    }
    Ok(
        sqlx::query("UPDATE durable_steps SET output=$2 WHERE id=$1 AND status='completed'")
            .bind(step_id)
            .bind(output)
            .execute(&mut **tx)
            .await?
            .rows_affected()
            == 1,
    )
}

/// Claim one step and create its sole attempt record. Duplicate delivery of the
/// same live claim does not run a handler twice; expired claims become failed
/// historical attempts before a successor claim is recorded.
pub async fn begin_step(
    tx: &mut Transaction<'_, Postgres>,
    step_id: Uuid,
    lease: &JobLease,
) -> Result<Option<i32>, sqlx::Error> {
    let Some(run_status) = lock_step(tx, step_id).await? else {
        return Ok(None);
    };
    if !matches!(run_status.as_str(), "queued" | "running" | "waiting")
        || !live_lease(tx, step_id, lease).await?
    {
        return Ok(None);
    }
    let step = sqlx::query(
        "SELECT status,claimed_job_id,claimed_job_attempt,claimed_by FROM durable_steps WHERE id=$1")
        .bind(step_id).fetch_one(&mut **tx).await?;
    let status: String = step.try_get("status")?;
    if !matches!(
        status.as_str(),
        "pending" | "queued" | "waiting" | "running" | "failed"
    ) {
        return Ok(None);
    }
    if status == "running" {
        let old_job: Option<Uuid> = step.try_get("claimed_job_id")?;
        let old_attempt: Option<i32> = step.try_get("claimed_job_attempt")?;
        let old_owner: Option<String> = step.try_get("claimed_by")?;
        if old_job == Some(lease.id)
            && old_attempt == Some(lease.attempt)
            && old_owner.as_deref() == Some(lease.owner.as_str())
        {
            return Ok(None);
        }
        let old_live: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM jobs WHERE id=$1 AND attempts=$2
             AND state='running' AND lease_owner=$3 AND leased_until>clock_timestamp())",
        )
        .bind(old_job)
        .bind(old_attempt)
        .bind(old_owner)
        .fetch_one(&mut **tx)
        .await?;
        if old_live {
            return Ok(None);
        }
        sqlx::query(
            "UPDATE durable_attempts SET status='failed',finished_at=now(),error='lease expired'
                     WHERE step_id=$1 AND status='running'",
        )
        .bind(step_id)
        .execute(&mut **tx)
        .await?;
    }
    let attempts: i32 = sqlx::query_scalar(
        "UPDATE durable_steps SET status='running',attempts=attempts+1,claimed_by=$2,
           claimed_job_id=$3,claimed_job_attempt=$4,started_at=COALESCE(started_at,now()),
           finished_at=NULL,error=NULL WHERE id=$1 RETURNING attempts",
    )
    .bind(step_id)
    .bind(&lease.owner)
    .bind(lease.id)
    .bind(lease.attempt)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO durable_attempts(project_id,run_id,step_id,attempt_number,status,worker_id,job_id,job_attempt)
         SELECT project_id,run_id,id,$2,'running',$3,$4,$5 FROM durable_steps WHERE id=$1")
        .bind(step_id).bind(attempts).bind(&lease.owner).bind(lease.id).bind(lease.attempt)
        .execute(&mut **tx).await?;
    sqlx::query(
        "UPDATE durable_runs SET status='running',started_at=COALESCE(started_at,now()),error=NULL
                 WHERE id=(SELECT run_id FROM durable_steps WHERE id=$1)",
    )
    .bind(step_id)
    .execute(&mut **tx)
    .await?;
    Ok(Some(attempts))
}

async fn transition_step(
    tx: &mut Transaction<'_, Postgres>,
    step_id: Uuid,
    lease: &JobLease,
    status: &str,
    attempt_status: &str,
    error: Option<&str>,
    output: Option<&Value>,
) -> Result<bool, sqlx::Error> {
    let Some(run_status) = lock_step(tx, step_id).await? else {
        return Ok(false);
    };
    if !matches!(run_status.as_str(), "queued" | "running" | "waiting")
        || !live_lease(tx, step_id, lease).await?
    {
        return Ok(false);
    }
    let changed = sqlx::query(
        "UPDATE durable_steps SET status=$5,error=$6,output=COALESCE($7,output),
           finished_at=CASE WHEN $5 IN ('queued','waiting') THEN NULL ELSE now() END,
           claimed_by=NULL,claimed_job_id=NULL,claimed_job_attempt=NULL
         WHERE id=$1 AND status='running' AND claimed_job_id=$2
           AND claimed_job_attempt=$3 AND claimed_by=$4",
    )
    .bind(step_id)
    .bind(lease.id)
    .bind(lease.attempt)
    .bind(&lease.owner)
    .bind(status)
    .bind(error)
    .bind(output)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if changed == 0 {
        return Ok(false);
    }
    sqlx::query("UPDATE durable_attempts SET status=$5,finished_at=now(),error=$6
                 WHERE step_id=$1 AND job_id=$2 AND job_attempt=$3 AND worker_id=$4 AND status='running'")
        .bind(step_id).bind(lease.id).bind(lease.attempt).bind(&lease.owner)
        .bind(attempt_status).bind(error).execute(&mut **tx).await?;
    Ok(true)
}

/// Record a handler outcome under the same live queue claim that began it.
pub async fn finish_step(
    tx: &mut Transaction<'_, Postgres>,
    step_id: Uuid,
    lease: &JobLease,
    status: &str,
    error: Option<&str>,
    output: Option<&Value>,
) -> Result<bool, sqlx::Error> {
    let attempt_status = match status {
        "queued" | "waiting" => "waiting",
        "completed" => "completed",
        "failed" => "failed",
        "blocked" => "blocked",
        "cancelled" => "cancelled",
        "skipped" => "skipped",
        _ => return Err(sqlx::Error::Protocol("invalid durable step outcome".into())),
    };
    transition_step(tx, step_id, lease, status, attempt_status, error, output).await
}

/// Queue retry keeps the step identity and closes the previous attempt.
pub async fn retry_step(
    tx: &mut Transaction<'_, Postgres>,
    step_id: Uuid,
    lease: &JobLease,
    error: Option<&str>,
) -> Result<bool, sqlx::Error> {
    transition_step(tx, step_id, lease, "queued", "failed", error, None).await
}

/// Dead-job recovery never overwrites another pending job or a live claim.
pub async fn fail_unclaimed_step(
    tx: &mut Transaction<'_, Postgres>,
    step_id: Uuid,
    error: &str,
) -> Result<bool, sqlx::Error> {
    let Some(run_status) = lock_step(tx, step_id).await? else {
        return Ok(false);
    };
    if !matches!(run_status.as_str(), "queued" | "running" | "waiting") {
        return Ok(false);
    }
    let changed = sqlx::query(
        "UPDATE durable_steps s SET status='failed',error=$2,finished_at=now(),
           claimed_by=NULL,claimed_job_id=NULL,claimed_job_attempt=NULL
         WHERE s.id=$1 AND s.status IN ('pending','queued','running','waiting')
         AND NOT EXISTS(SELECT 1 FROM jobs j WHERE j.kind='durable_run_step'
           AND j.project_id=s.project_id AND j.payload->>'run_id'=s.run_id::text
           AND j.payload->>'handler_kind'=s.handler_kind
           AND j.payload->>'step_id'=s.id::text AND j.state IN ('queued','running'))
         AND NOT EXISTS(SELECT 1 FROM jobs j WHERE j.id=s.claimed_job_id
           AND j.attempts=s.claimed_job_attempt AND j.state='running' AND j.leased_until>clock_timestamp())")
        .bind(step_id).bind(error).execute(&mut **tx).await?.rows_affected();
    if changed == 0 {
        return Ok(false);
    }
    sqlx::query(
        "UPDATE durable_attempts SET status='failed',error=$2,finished_at=now()
                 WHERE step_id=$1 AND status='running'",
    )
    .bind(step_id)
    .bind(error)
    .execute(&mut **tx)
    .await?;
    Ok(true)
}

/// Reconcile terminal queue claims independently of the handler process. The
/// sweep includes older dead jobs so a crash between queue recovery and this
/// transaction cannot strand a review generation forever.
pub async fn settle_terminal_step_jobs(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    let rows = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT DISTINCT s.id,CASE WHEN j.attempts>=j.max_attempts
             THEN 'job attempt budget exhausted'
             ELSE COALESCE(j.last_error,'durable step job terminated') END
         FROM jobs j JOIN durable_steps s
         ON j.payload->>'step_id'=s.id::text AND j.project_id=s.project_id
         AND j.payload->>'run_id'=s.run_id::text
         AND j.payload->>'handler_kind'=s.handler_kind
         JOIN durable_runs r ON r.id=s.run_id
         WHERE j.kind='durable_run_step' AND j.state IN ('dead','failed')
           AND s.status IN ('pending','queued','running','waiting')
           AND r.status IN ('queued','running','waiting')
           AND (s.claimed_job_id IS NULL OR s.claimed_job_id=j.id)
           AND NOT EXISTS(SELECT 1 FROM jobs active WHERE active.project_id=s.project_id
             AND active.kind='durable_run_step' AND active.payload->>'run_id'=s.run_id::text
             AND active.payload->>'handler_kind'=s.handler_kind
             AND active.payload->>'step_id'=s.id::text AND active.state IN ('queued','running'))
         LIMIT 100",
    )
    .fetch_all(pool)
    .await?;
    for (step_id, error) in rows {
        let mut tx = pool.begin().await?;
        if !fail_unclaimed_step(&mut tx, step_id, &error).await? {
            tx.commit().await?;
            continue;
        }
        let run_id = sqlx::query_scalar::<_, Uuid>("SELECT run_id FROM durable_steps WHERE id=$1")
            .bind(step_id)
            .fetch_one(&mut *tx)
            .await?;
        finish_run(&mut tx, run_id, "failed", Some(&error)).await?;
        sqlx::query(
            "UPDATE review_run_manifests SET state_code='review_execution_failed',
            state_message=$2 WHERE automation_run_id=$1",
        )
        .bind(run_id)
        .bind(&error)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE jobs SET state='dead',lease_owner=NULL,leased_until=NULL,lease_renewed_at=NULL,
            last_error='durable run failed' WHERE kind='durable_run_step'
            AND payload->>'run_id'=$1 AND state='queued'",
        )
        .bind(run_id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
    }
    Ok(())
}

/// Typed planners decide the terminal outcome; the shared lifecycle owns the
/// transition and refuses to resurrect cancellation or another terminal result.
pub async fn finish_run(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    status: &str,
    error: Option<&str>,
) -> Result<bool, sqlx::Error> {
    if !matches!(status, "completed" | "failed" | "blocked" | "cancelled") {
        return Err(sqlx::Error::Protocol("invalid durable run outcome".into()));
    }
    sqlx::query("SELECT id FROM durable_runs WHERE id=$1 FOR UPDATE")
        .bind(run_id)
        .fetch_optional(&mut **tx)
        .await?;
    let changed = sqlx::query(
        "UPDATE durable_runs SET status=$2,error=$3,started_at=COALESCE(started_at,now()),
        finished_at=now() WHERE id=$1 AND status IN ('queued','running','waiting')
        AND ($2<>'completed' OR NOT EXISTS(SELECT 1 FROM durable_steps
          WHERE run_id=$1 AND status IN ('pending','queued','running','waiting')))",
    )
    .bind(run_id)
    .bind(status)
    .bind(error)
    .execute(&mut **tx)
    .await?
    .rows_affected()
        == 1;
    if changed && status != "completed" {
        sqlx::query(
            "UPDATE durable_steps SET status='cancelled',finished_at=now(),
            claimed_by=NULL,claimed_job_id=NULL,claimed_job_attempt=NULL WHERE run_id=$1
            AND status IN ('pending','queued','running','waiting')",
        )
        .bind(run_id)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "UPDATE durable_attempts SET status='cancelled',finished_at=now()
            WHERE run_id=$1 AND status='running'",
        )
        .bind(run_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(changed)
}

pub async fn cancel_run(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    error: &str,
) -> Result<bool, sqlx::Error> {
    finish_run(tx, run_id, "cancelled", Some(error)).await
}

/// Purpose-built recipes retry their existing identity; review generations are
/// intentionally not reopened here.
pub async fn reopen_recipe_run(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let changed = sqlx::query(
        "UPDATE durable_runs SET status='queued',started_at=NULL,finished_at=NULL,error=NULL
        WHERE id=$1 AND kind='automation' AND status='failed'
        AND NOT EXISTS(SELECT 1 FROM review_run_manifests WHERE automation_run_id=$1)",
    )
    .bind(run_id)
    .execute(&mut **tx)
    .await?
    .rows_affected()
        == 1;
    if changed {
        sqlx::query(
            "UPDATE durable_steps SET status='queued',finished_at=NULL,error=NULL,
            claimed_by=NULL,claimed_job_id=NULL,claimed_job_attempt=NULL
            WHERE run_id=$1 AND status IN ('failed','cancelled')",
        )
        .bind(run_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(changed)
}
