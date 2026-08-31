use anyhow::{Result, ensure};
use deepref_application::{
    AutomationDefinitionStatus, AutomationRunStatus, AutomationStepRunStatus,
    AutomationTriggerKind, BuiltInAutomationRecipe, ConfigureAutomationDefinition,
    DispatchAutomationTrigger, StartAutomationManually,
};
use deepref_domain::{Actor, ActorKind, ProjectId};
use deepref_postgres::{
    AutomationError, AutomationFinalization, begin_next_automation_step, complete_automation_step,
    configure_automation_definition, dispatch_automation_trigger, fail_automation_step,
    finalize_automation_run, get_automation_run, list_automation_definitions, list_automation_runs,
    migrate, recover_expired_jobs, retry_automation_run, start_automation_manually,
};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use std::time::Duration;
use uuid::Uuid;

static DATABASE_TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(12)
        .connect(&url)
        .await
        .expect("DATABASE_URL database must be reachable");
    migrate(&pool)
        .await
        .expect("DATABASE_URL migrations must apply");
    Some(pool)
}

fn actor() -> Actor {
    Actor::new(ActorKind::User, "automation-test-user").expect("valid test actor")
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

async fn cleanup(pool: &PgPool, project_ids: &[ProjectId]) -> Result<()> {
    let ids = project_ids
        .iter()
        .map(|project_id| project_id.as_uuid())
        .collect::<Vec<_>>();
    sqlx::query("DELETE FROM projects WHERE id = ANY($1)")
        .bind(ids)
        .execute(pool)
        .await?;
    Ok(())
}

async fn configure_manual(
    pool: &PgPool,
    project_id: ProjectId,
) -> Result<deepref_application::AutomationDefinition> {
    Ok(configure_automation_definition(
        pool,
        &ConfigureAutomationDefinition::new(
            project_id,
            "Project maintenance",
            AutomationTriggerKind::Manual,
            BuiltInAutomationRecipe::ProjectMaintenanceV1,
            AutomationDefinitionStatus::Active,
            actor(),
        )?,
    )
    .await?)
}

async fn claim_automation_job(
    pool: &PgPool,
    owner: &str,
    job_id: Uuid,
    lease: Duration,
) -> Result<deepref_application::jobs::ClaimedJob> {
    let row = sqlx::query(
        "UPDATE jobs
         SET state='running', lease_owner=$2,
             leased_until=now()+($3 * interval '1 millisecond'),
             lease_renewed_at=now(), attempts=attempts+1
         WHERE id=$1 AND state='queued' AND available_at <= now()
         RETURNING id,project_id,kind,payload,attempts,max_attempts",
    )
    .bind(job_id)
    .bind(owner)
    .bind(lease.as_millis() as i64)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| anyhow::anyhow!("expected an automation job to be claimable"))?;
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
async fn manual_start_is_idempotent_and_persists_typed_recipe_steps() -> Result<()> {
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool, "automation idempotency fixture").await;
    let result = async {
        let definition = configure_manual(&pool, project_id).await?;
        let request = StartAutomationManually::new(
            project_id,
            definition.id.as_uuid(),
            "manual-request-1",
            actor(),
        )?;
        let first = start_automation_manually(&pool, &request).await?;
        let second = start_automation_manually(&pool, &request).await?;
        ensure!(first.created);
        ensure!(!second.created);
        ensure!(first.run_id == second.run_id);
        ensure!(first.job_id == second.job_id);

        let stored = get_automation_run(&pool, project_id, first.run_id).await?;
        ensure!(stored.status == AutomationRunStatus::Queued);
        ensure!(stored.steps.len() == 1);
        ensure!(stored.steps[0].ordinal == 0);
        ensure!(stored.steps[0].key == "recompute_project_metrics");
        ensure!(
            stored.steps[0].kind == deepref_application::AutomationStepKind::DeterministicAction
        );
        ensure!(stored.job.id == first.job_id);
        let executor_columns: i64 = sqlx::query_scalar(
            "SELECT count(*)::bigint
             FROM information_schema.columns
             WHERE table_schema=current_schema()
               AND table_name IN ('automation_definition_steps', 'automation_step_runs')
               AND column_name='executor'",
        )
        .fetch_one(&pool)
        .await?;
        ensure!(executor_columns == 0);

        let definitions = list_automation_definitions(&pool, project_id).await?;
        ensure!(definitions.len() == 1);
        ensure!(definitions[0].steps.len() == 1);
        let run_count: i64 = sqlx::query_scalar(
            "SELECT count(*)::bigint FROM automation_runs
             WHERE project_id=$1 AND definition_id=$2",
        )
        .bind(project_id.as_uuid())
        .bind(definition.id.as_uuid())
        .fetch_one(&pool)
        .await?;
        let job_count: i64 =
            sqlx::query_scalar("SELECT count(*)::bigint FROM jobs WHERE project_id=$1 AND id=$2")
                .bind(project_id.as_uuid())
                .bind(first.job_id)
                .fetch_one(&pool)
                .await?;
        let step_count: i64 = sqlx::query_scalar(
            "SELECT count(*)::bigint FROM automation_step_runs
             WHERE project_id=$1 AND automation_run_id=$2",
        )
        .bind(project_id.as_uuid())
        .bind(first.run_id.as_uuid())
        .fetch_one(&pool)
        .await?;
        ensure!(run_count == 1);
        ensure!(job_count == 1);
        ensure!(step_count == 1);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &[project_id]).await?;
    result
}

#[tokio::test]
async fn concurrent_known_trigger_dispatch_creates_one_run_job_and_step_set() -> Result<()> {
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool, "automation concurrent fixture").await;
    let result = async {
        let definition = configure_manual(&pool, project_id).await?;
        let request = DispatchAutomationTrigger::new(
            project_id,
            definition.id.as_uuid(),
            AutomationTriggerKind::Manual,
            None,
            "concurrent-request-1",
            actor(),
        )?;
        let (left, right) = tokio::join!(
            dispatch_automation_trigger(&pool, &request),
            dispatch_automation_trigger(&pool, &request),
        );
        let left = left?;
        let right = right?;
        ensure!(left.run_id == right.run_id);
        ensure!(left.job_id == right.job_id);
        ensure!(usize::from(left.created) + usize::from(right.created) == 1);

        let counts = sqlx::query(
            "SELECT
               (SELECT count(*) FROM automation_runs WHERE project_id=$1 AND id=$2) AS runs,
               (SELECT count(*) FROM jobs WHERE project_id=$1 AND id=$3) AS jobs,
               (SELECT count(*) FROM automation_step_runs WHERE project_id=$1 AND automation_run_id=$2) AS steps",
        )
        .bind(project_id.as_uuid())
        .bind(left.run_id.as_uuid())
        .bind(left.job_id)
        .fetch_one(&pool)
        .await?;
        ensure!(counts.get::<i64, _>("runs") == 1);
        ensure!(counts.get::<i64, _>("jobs") == 1);
        ensure!(counts.get::<i64, _>("steps") == 1);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &[project_id]).await?;
    result
}

#[tokio::test]
async fn expired_job_recovery_reclaims_running_step_and_rejects_zombie_completion() -> Result<()> {
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool, "automation lease recovery fixture").await;
    let result = async {
        let definition = configure_manual(&pool, project_id).await?;
        let dispatch = start_automation_manually(
            &pool,
            &StartAutomationManually::new(
                project_id,
                definition.id.as_uuid(),
                "lease-recovery-request-1",
                actor(),
            )?,
        )
        .await?;
        claim_automation_job(&pool, "worker-a", dispatch.job_id, Duration::from_secs(10)).await?;
        let first = begin_next_automation_step(&pool, project_id, dispatch.run_id, "worker-a")
            .await?
            .expect("owner A starts the step");
        ensure!(first.attempts == 1);
        ensure!(matches!(
            begin_next_automation_step(&pool, project_id, dispatch.run_id, "worker-b").await,
            Err(AutomationError::WorkerOwnership)
        ));

        sqlx::query(
            "UPDATE jobs
             SET leased_until=now() - interval '1 second'
             WHERE project_id=$1 AND id=$2",
        )
        .bind(project_id.as_uuid())
        .bind(dispatch.job_id)
        .execute(&pool)
        .await?;
        ensure!(recover_expired_jobs(&pool).await? == 1);
        claim_automation_job(&pool, "worker-b", dispatch.job_id, Duration::from_secs(10)).await?;

        let resumed = begin_next_automation_step(&pool, project_id, dispatch.run_id, "worker-b")
            .await?
            .expect("owner B reclaims the stale step");
        ensure!(resumed.id == first.id);
        ensure!(resumed.attempts == 2);
        ensure!(resumed.claimed_by.as_deref() == Some("worker-b"));
        ensure!(matches!(
            complete_automation_step(&pool, project_id, first.id, "worker-a").await,
            Err(AutomationError::WorkerOwnership)
        ));
        ensure!(complete_automation_step(&pool, project_id, resumed.id, "worker-b").await?);

        let row = sqlx::query(
            "SELECT count(*)::bigint AS rows, count(DISTINCT id)::bigint AS effects,
                    min(attempts) AS attempts, min(status) AS status
             FROM automation_step_runs
             WHERE project_id=$1 AND automation_run_id=$2",
        )
        .bind(project_id.as_uuid())
        .bind(dispatch.run_id.as_uuid())
        .fetch_one(&pool)
        .await?;
        ensure!(row.get::<i64, _>("rows") == 1);
        ensure!(row.get::<i64, _>("effects") == 1);
        ensure!(row.get::<i32, _>("attempts") == 2);
        ensure!(row.get::<String, _>("status") == "completed");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &[project_id]).await?;
    result
}

#[tokio::test]
async fn concurrent_step_begin_under_one_lease_has_one_effective_claim() -> Result<()> {
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool, "automation concurrent begin fixture").await;
    let result = async {
        let definition = configure_manual(&pool, project_id).await?;
        let dispatch = start_automation_manually(
            &pool,
            &StartAutomationManually::new(
                project_id,
                definition.id.as_uuid(),
                "concurrent-begin-request-1",
                actor(),
            )?,
        )
        .await?;
        claim_automation_job(&pool, "worker-a", dispatch.job_id, Duration::from_secs(10)).await?;
        let (left, right) = tokio::join!(
            begin_next_automation_step(&pool, project_id, dispatch.run_id, "worker-a"),
            begin_next_automation_step(&pool, project_id, dispatch.run_id, "worker-a"),
        );
        let left = left?.is_some();
        let right = right?.is_some();
        ensure!(usize::from(left) + usize::from(right) == 1);
        let row = sqlx::query(
            "SELECT count(*)::bigint AS rows, max(attempts) AS attempts,
                    max(claimed_job_attempt) AS job_attempt
             FROM automation_step_runs
             WHERE project_id=$1 AND automation_run_id=$2",
        )
        .bind(project_id.as_uuid())
        .bind(dispatch.run_id.as_uuid())
        .fetch_one(&pool)
        .await?;
        ensure!(row.get::<i64, _>("rows") == 1);
        ensure!(row.get::<i32, _>("attempts") == 1);
        ensure!(row.get::<i32, _>("job_attempt") == 1);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &[project_id]).await?;
    result
}

#[tokio::test]
async fn known_non_manual_trigger_persists_reference_and_paused_definition_is_rejected()
-> Result<()> {
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool, "automation trigger fixture").await;
    let result = async {
        let request = ConfigureAutomationDefinition::new(
            project_id,
            "Report maintenance",
            AutomationTriggerKind::ReportAdded,
            BuiltInAutomationRecipe::ProjectMaintenanceV1,
            AutomationDefinitionStatus::Active,
            actor(),
        )?;
        let definition = configure_automation_definition(&pool, &request).await?;
        ensure!(definition.created_at <= definition.updated_at);
        let dispatch = DispatchAutomationTrigger::new(
            project_id,
            definition.id.as_uuid(),
            AutomationTriggerKind::ReportAdded,
            Some("report:report-1".to_owned()),
            "report-added-request-1",
            actor(),
        )?;
        let created = dispatch_automation_trigger(&pool, &dispatch).await?;
        ensure!(created.created);
        let stored = get_automation_run(&pool, project_id, created.run_id).await?;
        ensure!(stored.trigger == AutomationTriggerKind::ReportAdded);
        ensure!(
            stored
                .trigger_reference
                .as_ref()
                .map(|reference| reference.as_str())
                == Some("report:report-1")
        );

        let paused = ConfigureAutomationDefinition::new(
            project_id,
            "Report maintenance",
            AutomationTriggerKind::ReportAdded,
            BuiltInAutomationRecipe::ProjectMaintenanceV1,
            AutomationDefinitionStatus::Paused,
            actor(),
        )?;
        configure_automation_definition(&pool, &paused).await?;
        let rejected = DispatchAutomationTrigger::new(
            project_id,
            definition.id.as_uuid(),
            AutomationTriggerKind::ReportAdded,
            Some("report:report-2".to_owned()),
            "report-added-request-2",
            actor(),
        )?;
        ensure!(dispatch_automation_trigger(&pool, &rejected).await.is_err());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &[project_id]).await?;
    result
}

#[tokio::test]
async fn failed_manual_run_retries_only_incomplete_steps_and_completed_retry_is_noop() -> Result<()>
{
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool, "automation retry fixture").await;
    let result = async {
        let definition = configure_manual(&pool, project_id).await?;
        sqlx::query(
            "INSERT INTO automation_definition_steps
             (project_id, definition_id, ordinal, step_key, step_kind)
             VALUES ($1,$2,1,'second_controlled_step','deterministic_action')",
        )
        .bind(project_id.as_uuid())
        .bind(definition.id.as_uuid())
        .execute(&pool)
        .await?;
        let dispatch = start_automation_manually(
            &pool,
            &StartAutomationManually::new(
                project_id,
                definition.id.as_uuid(),
                "retry-request-1",
                actor(),
            )?,
        )
        .await?;
        let claimed_a =
            claim_automation_job(&pool, "worker-a", dispatch.job_id, Duration::from_secs(10))
                .await?;
        ensure!(claimed_a.attempts == 1);
        let first_step = begin_next_automation_step(&pool, project_id, dispatch.run_id, "worker-a")
            .await?
            .expect("claimed run has a first step");
        ensure!(first_step.status == AutomationStepRunStatus::Running);
        ensure!(first_step.attempts == 1);
        ensure!(complete_automation_step(&pool, project_id, first_step.id, "worker-a").await?);
        let second_step =
            begin_next_automation_step(&pool, project_id, dispatch.run_id, "worker-a")
                .await?
                .expect("claimed run has a second step");
        ensure!(second_step.ordinal == 1);
        ensure!(second_step.attempts == 1);
        ensure!(
            fail_automation_step(
                &pool,
                project_id,
                second_step.id,
                "worker-a",
                "deterministic action failed",
            )
            .await?
        );
        ensure!(
            finalize_automation_run(&pool, project_id, dispatch.run_id).await?
                == AutomationFinalization::Failed
        );
        let attempts_before_retry: i32 =
            sqlx::query_scalar("SELECT attempts FROM jobs WHERE project_id=$1 AND id=$2")
                .bind(project_id.as_uuid())
                .bind(dispatch.job_id)
                .fetch_one(&pool)
                .await?;
        ensure!(retry_automation_run(&pool, project_id, dispatch.run_id).await?);
        let retry_job =
            sqlx::query("SELECT state, attempts FROM jobs WHERE project_id=$1 AND id=$2")
                .bind(project_id.as_uuid())
                .bind(dispatch.job_id)
                .fetch_one(&pool)
                .await?;
        ensure!(retry_job.get::<String, _>("state") == "queued");
        ensure!(retry_job.get::<i32, _>("attempts") == attempts_before_retry);

        let claimed_b =
            claim_automation_job(&pool, "worker-b", dispatch.job_id, Duration::from_secs(10))
                .await?;
        ensure!(claimed_b.attempts == attempts_before_retry + 1);
        let resumed = begin_next_automation_step(&pool, project_id, dispatch.run_id, "worker-b")
            .await?
            .expect("retry resumes the failed step");
        ensure!(resumed.id == second_step.id);
        ensure!(resumed.attempts == 2);
        let after_begin = get_automation_run(&pool, project_id, dispatch.run_id).await?;
        ensure!(after_begin.steps[0].status == AutomationStepRunStatus::Completed);
        ensure!(after_begin.steps[0].attempts == 1);
        ensure!(complete_automation_step(&pool, project_id, resumed.id, "worker-b").await?);
        ensure!(
            finalize_automation_run(&pool, project_id, dispatch.run_id).await?
                == AutomationFinalization::Completed
        );
        ensure!(!complete_automation_step(&pool, project_id, resumed.id, "worker-b").await?);
        ensure!(!retry_automation_run(&pool, project_id, dispatch.run_id).await?);
        ensure!(
            begin_next_automation_step(&pool, project_id, dispatch.run_id, "worker-b")
                .await?
                .is_none()
        );
        ensure!(
            finalize_automation_run(&pool, project_id, dispatch.run_id).await?
                == AutomationFinalization::Completed
        );

        let stored = get_automation_run(&pool, project_id, dispatch.run_id).await?;
        ensure!(stored.status == AutomationRunStatus::Completed);
        ensure!(stored.steps[0].status == AutomationStepRunStatus::Completed);
        ensure!(stored.steps[0].attempts == 1);
        ensure!(stored.steps[1].status == AutomationStepRunStatus::Completed);
        ensure!(stored.steps[1].attempts == 2);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &[project_id]).await?;
    result
}

#[tokio::test]
async fn project_scoping_hides_runs_and_rejects_cross_project_definition_dispatch() -> Result<()> {
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let first_project = project(&pool, "automation isolation one").await;
    let second_project = project(&pool, "automation isolation two").await;
    let result = async {
        let first_definition = configure_manual(&pool, first_project).await?;
        let second_definition = configure_manual(&pool, second_project).await?;
        let first_run = start_automation_manually(
            &pool,
            &StartAutomationManually::new(
                first_project,
                first_definition.id.as_uuid(),
                "isolation-request-1",
                actor(),
            )?,
        )
        .await?;
        ensure!(
            list_automation_runs(&pool, second_project, 100)
                .await?
                .is_empty()
        );
        ensure!(matches!(
            get_automation_run(&pool, second_project, first_run.run_id).await,
            Err(AutomationError::RunNotFound)
        ));

        let cross_project = DispatchAutomationTrigger::new(
            second_project,
            first_definition.id.as_uuid(),
            AutomationTriggerKind::Manual,
            None,
            "cross-project-request-1",
            actor(),
        )?;
        ensure!(
            dispatch_automation_trigger(&pool, &cross_project)
                .await
                .is_err()
        );
        let second_definitions = list_automation_definitions(&pool, second_project).await?;
        ensure!(second_definitions.len() == 1);
        ensure!(second_definitions[0].id.as_uuid() == second_definition.id.as_uuid());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &[first_project, second_project]).await?;
    result
}

#[tokio::test]
async fn run_detail_aggregates_ai_usage_and_database_rejects_invalid_transitions() -> Result<()> {
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool, "automation usage fixture").await;
    let result = async {
        let definition = configure_manual(&pool, project_id).await?;
        let dispatch = start_automation_manually(
            &pool,
            &StartAutomationManually::new(
                project_id,
                definition.id.as_uuid(),
                "usage-request-1",
                actor(),
            )?,
        )
        .await?;
        for (input_tokens, output_tokens, cost_micros) in [(11_i64, 7_i64, 13_i64), (5, 3, 2)] {
            sqlx::query(
                "INSERT INTO ai_runs
                 (id,project_id,task_kind,provider,model,profile,model_version,parameters,
                  prompt_version,prompt_hash,schema_version,schema_hash,input_hash,reuse_hash,
                  evidence_refs,input_tokens,output_tokens,cost_micros,output,status,
                  parent_automation_run_id,completed_at)
                 VALUES ($1,$2,'study_design_classification','test-provider','test-model',
                         'test-profile','test-model-v1','{}'::jsonb,'automation-test.v1',
                         repeat('a',64),'automation-test.schema.v1',repeat('b',64),repeat('c',64),
                         repeat('d',64),'[]'::jsonb,$3,$4,$5,'{}'::jsonb,'completed',$6,now())",
            )
            .bind(Uuid::new_v4())
            .bind(project_id.as_uuid())
            .bind(input_tokens)
            .bind(output_tokens)
            .bind(cost_micros)
            .bind(dispatch.run_id.as_uuid())
            .execute(&pool)
            .await?;
        }
        let stored = get_automation_run(&pool, project_id, dispatch.run_id).await?;
        ensure!(stored.usage.input_tokens == 16);
        ensure!(stored.usage.output_tokens == 10);
        ensure!(stored.usage.cost_micros == 15);
        let invalid_run_transition = sqlx::query(
            "UPDATE automation_runs
             SET status='completed', started_at=now(), finished_at=now()
             WHERE project_id=$1 AND id=$2",
        )
        .bind(project_id.as_uuid())
        .bind(dispatch.run_id.as_uuid())
        .execute(&pool)
        .await;
        ensure!(invalid_run_transition.is_err());
        let step_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM automation_step_runs WHERE project_id=$1 AND automation_run_id=$2",
        )
        .bind(project_id.as_uuid())
        .bind(dispatch.run_id.as_uuid())
        .fetch_one(&pool)
        .await?;
        let invalid_step_transition = sqlx::query(
            "UPDATE automation_step_runs
             SET status='completed', started_at=now(), finished_at=now()
             WHERE project_id=$1 AND id=$2",
        )
        .bind(project_id.as_uuid())
        .bind(step_id)
        .execute(&pool)
        .await;
        ensure!(invalid_step_transition.is_err());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &[project_id]).await?;
    result
}

#[tokio::test]
async fn deleting_automation_parent_or_definition_preserves_ai_audit_and_project_identity()
-> Result<()> {
    let _guard = DATABASE_TEST_MUTEX.lock().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool, "automation audit fk fixture").await;
    let result = async {
        let definition = configure_manual(&pool, project_id).await?;
        let dispatch = start_automation_manually(
            &pool,
            &StartAutomationManually::new(
                project_id,
                definition.id.as_uuid(),
                "audit-fk-request-1",
                actor(),
            )?,
        )
        .await?;
        let ai_run_id = Uuid::new_v4();
        sqlx::query(
            r#"INSERT INTO ai_runs
             (id,project_id,task_kind,provider,model,profile,model_version,parameters,
              prompt_version,prompt_hash,schema_version,schema_hash,input_hash,reuse_hash,
              evidence_refs,input_tokens,output_tokens,cost_micros,output,status,
              parent_automation_run_id,completed_at)
             VALUES ($1,$2,'automation_agent','test-provider','test-model',
                     'test-profile','test-model-v1','{}'::jsonb,'automation-test.v1',
                     repeat('a',64),'automation-test.schema.v1',repeat('b',64),repeat('c',64),
                     repeat('d',64),'[]'::jsonb,1,2,3,'{"audit":true}'::jsonb,'completed',$3,now())"#,
        )
        .bind(ai_run_id)
        .bind(project_id.as_uuid())
        .bind(dispatch.run_id.as_uuid())
        .execute(&pool)
        .await?;
        let project_is_not_null: bool = sqlx::query_scalar(
            "SELECT is_nullable = 'NO'
             FROM information_schema.columns
             WHERE table_schema=current_schema() AND table_name='ai_runs'
               AND column_name='project_id'",
        )
        .fetch_one(&pool)
        .await?;
        ensure!(project_is_not_null);

        ensure!(
            sqlx::query("DELETE FROM jobs WHERE project_id=$1 AND id=$2")
                .bind(project_id.as_uuid())
                .bind(dispatch.job_id)
                .execute(&pool)
                .await
                .is_err()
        );
        sqlx::query("DELETE FROM automation_runs WHERE project_id=$1 AND id=$2")
            .bind(project_id.as_uuid())
            .bind(dispatch.run_id.as_uuid())
            .execute(&pool)
            .await?;
        let after_parent_delete = sqlx::query(
            "SELECT project_id, parent_automation_run_id, output, status
             FROM ai_runs WHERE id=$1",
        )
        .bind(ai_run_id)
        .fetch_one(&pool)
        .await?;
        ensure!(after_parent_delete.get::<Uuid, _>("project_id") == project_id.as_uuid());
        ensure!(
            after_parent_delete
                .get::<Option<Uuid>, _>("parent_automation_run_id")
                .is_none()
        );
        ensure!(after_parent_delete.get::<String, _>("status") == "completed");
        ensure!(after_parent_delete.get::<serde_json::Value, _>("output")["audit"] == true);

        sqlx::query("DELETE FROM automation_definitions WHERE project_id=$1 AND id=$2")
            .bind(project_id.as_uuid())
            .bind(definition.id.as_uuid())
            .execute(&pool)
            .await?;
        sqlx::query("DELETE FROM jobs WHERE project_id=$1 AND id=$2")
            .bind(project_id.as_uuid())
            .bind(dispatch.job_id)
            .execute(&pool)
            .await?;
        let audit_count: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_runs WHERE id=$1")
            .bind(ai_run_id)
            .fetch_one(&pool)
            .await?;
        ensure!(audit_count == 1);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &[project_id]).await?;
    result
}
