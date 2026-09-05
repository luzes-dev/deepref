#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use anyhow::{Result, ensure};
use deepref_application::{
    AutomationDefinitionStatus, AutomationRunId, AutomationRunStatus, AutomationStepRunStatus,
    AutomationTriggerKind, BuiltInAutomationRecipe, ConfigureAutomationDefinition,
    StartAutomationManually,
};
use deepref_domain::{Actor, ActorKind, ProjectId};
use deepref_postgres::{
    begin_next_automation_step, complete_job, configure_automation_definition, get_automation_run,
    migrate, recover_expired_jobs, start_automation_manually,
};
use deepref_worker::{delivery::DeliveryAction, processor::handle_job_with_documents_owned};
use serde_json::json;
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use std::time::Duration;
use uuid::Uuid;

static DATABASE_TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .ok()?;
    migrate(&pool).await.ok()?;
    Some(pool)
}

fn actor() -> Actor {
    Actor::new(ActorKind::System, "worker-automation-test").expect("valid test actor")
}

async fn project(pool: &PgPool, name: &str) -> ProjectId {
    let project_id = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,$2)")
        .bind(project_id.as_uuid())
        .bind(name)
        .execute(pool)
        .await
        .expect("project fixture inserts");
    project_id
}

async fn cleanup(pool: &PgPool, project_id: ProjectId) -> Result<()> {
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id.as_uuid())
        .execute(pool)
        .await?;
    Ok(())
}

async fn started_job(
    pool: &PgPool,
    project_id: ProjectId,
    key: &str,
) -> Result<(AutomationRunId, Uuid)> {
    let definition = configure_automation_definition(
        pool,
        &ConfigureAutomationDefinition::new(
            project_id,
            format!("Worker automation {key}"),
            AutomationTriggerKind::Manual,
            BuiltInAutomationRecipe::ProjectMaintenanceV1,
            AutomationDefinitionStatus::Active,
            actor(),
        )?,
    )
    .await?;
    let dispatch = start_automation_manually(
        pool,
        &StartAutomationManually::new(
            project_id,
            definition.id.as_uuid(),
            format!("worker-run-{key}"),
            actor(),
        )?,
    )
    .await?;
    Ok((dispatch.run_id, dispatch.job_id))
}

async fn claimed_job(
    pool: &PgPool,
    job_id: Uuid,
    owner: &str,
) -> Result<deepref_application::jobs::ClaimedJob> {
    let row = sqlx::query(
        "UPDATE jobs
         SET state='running', lease_owner=$2,
             leased_until=now()+interval '30 seconds',
             lease_renewed_at=now(), attempts=attempts+1
         WHERE id=$1 AND state='queued' AND available_at <= now()
         RETURNING id,project_id,kind,payload,attempts,max_attempts",
    )
    .bind(job_id)
    .bind(owner)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| anyhow::anyhow!("expected automation job to be claimable"))?;
    Ok(deepref_application::jobs::ClaimedJob {
        id: row.get("id"),
        project_id: ProjectId::new(row.get("project_id")),
        kind: row.get("kind"),
        payload: row.get("payload"),
        attempts: row.get("attempts"),
        max_attempts: row.get("max_attempts"),
    })
}

#[tokio::test]
async fn automation_job_executes_builtin_step_and_completed_retry_is_noop() -> Result<()> {
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool, "worker automation success").await;
    let result = async {
        let (run_id, job_id) = started_job(&pool, project_id, "success").await?;
        let owner = "worker-success";
        let claimed = claimed_job(&pool, job_id, owner).await?;

        ensure!(
            handle_job_with_documents_owned(
                pool.clone(),
                &claimed,
                owner,
                Duration::from_secs(30),
                None,
                None,
            )
            .await?
                == DeliveryAction::Ack
        );
        ensure!(
            handle_job_with_documents_owned(
                pool.clone(),
                &claimed,
                owner,
                Duration::from_secs(30),
                None,
                None,
            )
            .await?
                == DeliveryAction::Ack
        );
        ensure!(complete_job(&pool, owner, job_id).await?);

        let run = get_automation_run(&pool, project_id, run_id).await?;
        ensure!(run.status == AutomationRunStatus::Completed);
        ensure!(run.steps.len() == 1);
        ensure!(run.steps[0].status == AutomationStepRunStatus::Completed);
        ensure!(run.steps[0].attempts == 1);
        let job_state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id=$1")
            .bind(job_id)
            .fetch_one(&pool)
            .await?;
        ensure!(job_state == "completed");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, project_id).await?;
    result
}

#[tokio::test]
async fn swapped_run_payload_cannot_execute_another_same_owner_job() -> Result<()> {
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool, "worker automation payload binding").await;
    let result = async {
        let (run_a, job_a) = started_job(&pool, project_id, "payload-a").await?;
        let (run_b, job_b) = started_job(&pool, project_id, "payload-b").await?;
        let owner = "worker-shared-owner";
        let claimed_a = claimed_job(&pool, job_a, owner).await?;
        let claimed_b = claimed_job(&pool, job_b, owner).await?;

        let mut corrupted = claimed_a;
        corrupted.payload = json!({"automation_run_id": run_b.as_uuid()});
        let error = handle_job_with_documents_owned(
            pool.clone(),
            &corrupted,
            owner,
            Duration::from_secs(30),
            None,
            None,
        )
        .await;
        ensure!(error.is_err(), "swapped automation payload was accepted");

        let run_a_state = get_automation_run(&pool, project_id, run_a).await?;
        ensure!(run_a_state.status == AutomationRunStatus::Queued);
        ensure!(run_a_state.steps[0].status == AutomationStepRunStatus::Pending);
        ensure!(run_a_state.steps[0].attempts == 0);
        let run_b_state = get_automation_run(&pool, project_id, run_b).await?;
        ensure!(run_b_state.status == AutomationRunStatus::Queued);
        ensure!(run_b_state.steps[0].status == AutomationStepRunStatus::Pending);
        ensure!(run_b_state.steps[0].attempts == 0);
        let metric_snapshot_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM metric_snapshots WHERE project_id=$1")
                .bind(project_id.as_uuid())
                .fetch_one(&pool)
                .await?;
        ensure!(metric_snapshot_count == 0);

        // Keep the second lease live and assert the exact claimed job remains
        // independently addressable after the rejected payload.
        let claimed_job_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs
             WHERE project_id=$1 AND id=ANY($2) AND state='running'
               AND lease_owner=$3 AND leased_until > now()",
        )
        .bind(project_id.as_uuid())
        .bind(vec![job_a, job_b])
        .bind(owner)
        .fetch_one(&pool)
        .await?;
        ensure!(claimed_job_count == 2);
        let _ = claimed_b;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, project_id).await?;
    result
}

#[tokio::test]
async fn unknown_step_fails_closed_and_lease_owner_is_required() -> Result<()> {
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool, "worker automation policy").await;
    let result = async {
        let (run_id, job_id) = started_job(&pool, project_id, "unknown").await?;
        // The immutable snapshot trigger intentionally prevents this state in
        // production. Temporarily corrupt the durable snapshot only in this
        // fixture so the worker's fail-closed branch is exercised.
        let mut transaction = pool.begin().await?;
        sqlx::query(
            "ALTER TABLE automation_step_runs DISABLE TRIGGER automation_step_snapshot_trigger",
        )
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE automation_step_runs
             SET step_key='untrusted_command'
             WHERE project_id=$1 AND automation_run_id=$2",
        )
        .bind(project_id.as_uuid())
        .bind(run_id.as_uuid())
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "ALTER TABLE automation_step_runs ENABLE TRIGGER automation_step_snapshot_trigger",
        )
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;

        let claimed = claimed_job(&pool, job_id, "worker-policy").await?;
        let wrong_owner = handle_job_with_documents_owned(
            pool.clone(),
            &claimed,
            "another-worker",
            Duration::from_secs(30),
            None,
            None,
        )
        .await;
        ensure!(wrong_owner.is_err());

        ensure!(
            handle_job_with_documents_owned(
                pool.clone(),
                &claimed,
                "worker-policy",
                Duration::from_secs(30),
                None,
                None,
            )
            .await?
                == DeliveryAction::Terminate
        );
        let terminal = deepref_application::jobs::ClaimedJob {
            max_attempts: claimed.attempts,
            ..claimed
        };
        ensure!(
            deepref_postgres::fail_job(
                &pool,
                "worker-policy",
                &terminal,
                "terminal job failure",
                Duration::ZERO,
            )
            .await?
        );

        let run = get_automation_run(&pool, project_id, run_id).await?;
        ensure!(run.status == AutomationRunStatus::Failed);
        ensure!(run.steps[0].status == AutomationStepRunStatus::Failed);
        ensure!(
            run.steps[0].error.as_deref()
                == Some("automation step is not an accepted built-in deterministic action")
        );
        let job_state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id=$1")
            .bind(job_id)
            .fetch_one(&pool)
            .await?;
        ensure!(job_state == "dead");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, project_id).await?;
    result
}

#[tokio::test]
async fn crashed_step_is_reclaimed_by_new_owner_without_repeating_completed_steps() -> Result<()> {
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool, "worker automation crash retry").await;
    let result = async {
        let (run_id, job_id) = started_job(&pool, project_id, "crash").await?;
        let claimed_a = claimed_job(&pool, job_id, "worker-crashed").await?;
        let first = begin_next_automation_step(&pool, project_id, run_id, "worker-crashed")
            .await?
            .expect("first worker claims the step");
        ensure!(first.attempts == 1);

        sqlx::query(
            "UPDATE jobs SET leased_until=now() - interval '1 second'
             WHERE id=$1",
        )
        .bind(job_id)
        .execute(&pool)
        .await?;
        ensure!(recover_expired_jobs(&pool).await? == 1);
        let claimed_b = claimed_job(&pool, job_id, "worker-recovered").await?;
        ensure!(claimed_b.attempts == claimed_a.attempts + 1);
        ensure!(
            handle_job_with_documents_owned(
                pool.clone(),
                &claimed_b,
                "worker-recovered",
                Duration::from_secs(30),
                None,
                None,
            )
            .await?
                == DeliveryAction::Ack
        );
        ensure!(complete_job(&pool, "worker-recovered", job_id).await?);

        let run = get_automation_run(&pool, project_id, run_id).await?;
        ensure!(run.status == AutomationRunStatus::Completed);
        ensure!(run.steps[0].status == AutomationStepRunStatus::Completed);
        ensure!(run.steps[0].attempts == 2);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, project_id).await?;
    result
}
