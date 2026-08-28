use chrono::{DateTime, Utc};
use deepref_application::{
    AutomationDefinition, AutomationDefinitionId, AutomationDefinitionStatus,
    AutomationIdempotencyKey, AutomationJobStatus, AutomationJobVisibility, AutomationName,
    AutomationRun, AutomationRunId, AutomationRunStatus, AutomationStepKind, AutomationStepRun,
    AutomationStepRunId, AutomationStepRunStatus, AutomationStepSnapshot, AutomationTriggerKind,
    AutomationTriggerReference, AutomationUsage, AutomationValidationError,
    BuiltInAutomationRecipe, ConfigureAutomationDefinition, DispatchAutomationTrigger,
    StartAutomationManually, validate_error, validate_run_list_limit, validate_worker_id,
};
use deepref_domain::{Actor, ActorKind, ProjectId};
use serde::Deserialize;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum AutomationError {
    #[error("automation database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("automation JSON value is invalid: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("automation input is invalid: {0}")]
    InvalidInput(#[from] AutomationValidationError),
    #[error("automation definition was not found")]
    DefinitionNotFound,
    #[error("automation run was not found")]
    RunNotFound,
    #[error("automation step was not found")]
    StepNotFound,
    #[error("automation worker does not own the running step")]
    WorkerOwnership,
    #[error("automation run is not ready to finalize")]
    RunNotReady,
    #[error("automation run is already terminal")]
    RunAlreadyTerminal,
    #[error("automation run cannot be retried from its current state")]
    RunNotRetryable,
    #[error("stored automation value is invalid: {0}")]
    InvalidStoredValue(String),
    #[error("automation state transition is invalid: {0}")]
    InvalidTransition(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutomationDispatchResult {
    pub run_id: AutomationRunId,
    pub job_id: Uuid,
    pub created: bool,
}

pub async fn configure_automation_definition(
    pool: &PgPool,
    request: &ConfigureAutomationDefinition,
) -> Result<AutomationDefinition, AutomationError> {
    request.validate()?;
    let row = sqlx::query(
        "SELECT id, project_id, name, trigger_kind, recipe_id, recipe_version,
                status, actor_kind, actor_id, created_at, updated_at
         FROM configure_automation_definition($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(request.project_id.as_uuid())
    .bind(request.name.as_str())
    .bind(request.trigger.as_str())
    .bind(request.recipe.id())
    .bind(request.recipe.version())
    .bind(request.status.as_str())
    .bind(request.actor.kind().as_str())
    .bind(request.actor.id())
    .fetch_one(pool)
    .await?;
    let id: Uuid = row.get("id");
    let steps = load_definition_steps(pool, request.project_id, id).await?;
    definition_from_row(&row, steps)
}

pub async fn list_automation_definitions(
    pool: &PgPool,
    project_id: ProjectId,
) -> Result<Vec<AutomationDefinition>, AutomationError> {
    validate_project(project_id)?;
    let rows = sqlx::query(
        "SELECT id, project_id, name, trigger_kind, recipe_id, recipe_version,
                status, actor_kind, actor_id, created_at, updated_at, steps
         FROM list_automation_definitions($1)",
    )
    .bind(project_id.as_uuid())
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| definition_from_row(row, parse_definition_steps(row.get("steps"))?))
        .collect()
}

pub async fn dispatch_automation_trigger(
    pool: &PgPool,
    request: &DispatchAutomationTrigger,
) -> Result<AutomationDispatchResult, AutomationError> {
    request.validate()?;
    let row = sqlx::query(
        "SELECT run_id, job_id, created
         FROM dispatch_automation_trigger($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(request.project_id.as_uuid())
    .bind(request.definition_id.as_uuid())
    .bind(request.trigger.as_str())
    .bind(
        request
            .trigger_reference
            .as_ref()
            .map(AutomationTriggerReference::as_str),
    )
    .bind(request.idempotency_key.as_str())
    .bind(request.actor.kind().as_str())
    .bind(request.actor.id())
    .fetch_one(pool)
    .await?;
    Ok(AutomationDispatchResult {
        run_id: AutomationRunId::new(row.get("run_id"))?,
        job_id: row.get("job_id"),
        created: row.get("created"),
    })
}

pub async fn start_automation_manually(
    pool: &PgPool,
    request: &StartAutomationManually,
) -> Result<AutomationDispatchResult, AutomationError> {
    request.validate()?;
    let row = sqlx::query(
        "SELECT run_id, job_id, created
         FROM start_automation_manually($1,$2,$3,$4,$5)",
    )
    .bind(request.project_id.as_uuid())
    .bind(request.definition_id.as_uuid())
    .bind(request.idempotency_key.as_str())
    .bind(request.actor.kind().as_str())
    .bind(request.actor.id())
    .fetch_one(pool)
    .await?;
    Ok(AutomationDispatchResult {
        run_id: AutomationRunId::new(row.get("run_id"))?,
        job_id: row.get("job_id"),
        created: row.get("created"),
    })
}

pub async fn list_automation_runs(
    pool: &PgPool,
    project_id: ProjectId,
    limit: i64,
) -> Result<Vec<AutomationRun>, AutomationError> {
    validate_project(project_id)?;
    validate_run_list_limit(limit)?;
    let rows = sqlx::query("SELECT * FROM list_automation_runs($1,$2)")
        .bind(project_id.as_uuid())
        .bind(i32::try_from(limit).map_err(|_| {
            AutomationError::InvalidInput(AutomationValidationError::InvalidRunListLimit)
        })?)
        .fetch_all(pool)
        .await?;
    rows.iter().map(run_from_row).collect()
}

pub async fn get_automation_run(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: AutomationRunId,
) -> Result<AutomationRun, AutomationError> {
    validate_project(project_id)?;
    let row = sqlx::query("SELECT * FROM get_automation_run($1,$2)")
        .bind(project_id.as_uuid())
        .bind(run_id.as_uuid())
        .fetch_optional(pool)
        .await?
        .ok_or(AutomationError::RunNotFound)?;
    run_from_row(&row)
}

#[derive(Debug, Deserialize)]
struct StoredDefinitionStep {
    ordinal: i32,
    key: String,
    kind: String,
}

#[derive(Debug, Deserialize)]
struct StoredStepRun {
    id: Uuid,
    ordinal: i32,
    key: String,
    kind: String,
    status: String,
    attempts: i32,
    claimed_by: Option<String>,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    error: Option<String>,
}

fn definition_from_row(
    row: &PgRow,
    steps: Vec<AutomationStepSnapshot>,
) -> Result<AutomationDefinition, AutomationError> {
    Ok(AutomationDefinition {
        id: AutomationDefinitionId::new(row.get("id"))?,
        project_id: ProjectId::new(row.get("project_id")),
        name: AutomationName::new(row.get::<String, _>("name"))?,
        trigger: parse_trigger(&row.get::<String, _>("trigger_kind"))?,
        recipe: parse_recipe(
            &row.get::<String, _>("recipe_id"),
            row.get("recipe_version"),
        )?,
        status: parse_definition_status(&row.get::<String, _>("status"))?,
        actor: parse_actor(
            &row.get::<String, _>("actor_kind"),
            &row.get::<String, _>("actor_id"),
        )?,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        steps,
    })
}

async fn load_definition_steps(
    pool: &PgPool,
    project_id: ProjectId,
    definition_id: Uuid,
) -> Result<Vec<AutomationStepSnapshot>, AutomationError> {
    let rows = sqlx::query(
        "SELECT ordinal, step_key, step_kind
         FROM automation_definition_steps
         WHERE project_id=$1 AND definition_id=$2
         ORDER BY ordinal",
    )
    .bind(project_id.as_uuid())
    .bind(definition_id)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(AutomationStepSnapshot {
                ordinal: row.get("ordinal"),
                key: row.get("step_key"),
                kind: parse_step_kind(&row.get::<String, _>("step_kind"))?,
            })
        })
        .collect()
}

fn parse_definition_steps(value: Value) -> Result<Vec<AutomationStepSnapshot>, AutomationError> {
    serde_json::from_value::<Vec<StoredDefinitionStep>>(value)?
        .into_iter()
        .map(|step| {
            Ok(AutomationStepSnapshot {
                ordinal: step.ordinal,
                key: step.key,
                kind: parse_step_kind(&step.kind)?,
            })
        })
        .collect()
}

fn run_from_row(row: &PgRow) -> Result<AutomationRun, AutomationError> {
    let steps = serde_json::from_value::<Vec<StoredStepRun>>(row.get("steps"))?
        .into_iter()
        .map(|step| {
            Ok(AutomationStepRun {
                id: AutomationStepRunId::new(step.id)?,
                project_id: ProjectId::new(row.get("project_id")),
                run_id: AutomationRunId::new(row.get("run_id"))?,
                ordinal: step.ordinal,
                key: step.key,
                kind: parse_step_kind(&step.kind)?,
                status: parse_step_run_status(&step.status)?,
                attempts: step.attempts,
                claimed_by: step.claimed_by,
                started_at: step.started_at,
                finished_at: step.finished_at,
                error: step.error,
            })
        })
        .collect::<Result<Vec<_>, AutomationError>>()?;
    let run_id = AutomationRunId::new(row.get("run_id"))?;
    Ok(AutomationRun {
        id: run_id,
        project_id: ProjectId::new(row.get("project_id")),
        definition_id: AutomationDefinitionId::new(row.get("definition_id"))?,
        recipe: parse_recipe(
            &row.get::<String, _>("recipe_id"),
            row.get("recipe_version"),
        )?,
        trigger: parse_trigger(&row.get::<String, _>("trigger_kind"))?,
        trigger_reference: row
            .get::<Option<String>, _>("trigger_reference")
            .map(AutomationTriggerReference::new)
            .transpose()?,
        idempotency_key: AutomationIdempotencyKey::new(row.get::<String, _>("idempotency_key"))?,
        actor: parse_actor(
            &row.get::<String, _>("actor_kind"),
            &row.get::<String, _>("actor_id"),
        )?,
        status: parse_run_status(&row.get::<String, _>("status"))?,
        created_at: row.get("created_at"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
        error: row.get("run_error"),
        job: AutomationJobVisibility {
            id: row.get("job_id"),
            status: parse_job_status(&row.get::<String, _>("job_state"))?,
            attempts: row.get("job_attempts"),
            max_attempts: row.get("job_max_attempts"),
            available_at: row.get("job_available_at"),
            leased_until: row.get("job_leased_until"),
            last_error: row.get("job_last_error"),
        },
        steps,
        usage: AutomationUsage {
            input_tokens: row.get("input_tokens"),
            output_tokens: row.get("output_tokens"),
            cost_micros: row.get("cost_micros"),
        },
    })
}

fn parse_trigger(value: &str) -> Result<AutomationTriggerKind, AutomationError> {
    AutomationTriggerKind::parse(value)
        .ok_or_else(|| AutomationError::InvalidStoredValue(format!("unknown trigger {value}")))
}

fn parse_recipe(value: &str, version: i32) -> Result<BuiltInAutomationRecipe, AutomationError> {
    let recipe = format!("{value}.v{version}");
    BuiltInAutomationRecipe::parse(&recipe)
        .ok_or_else(|| AutomationError::InvalidStoredValue(format!("unknown recipe {recipe}")))
}

fn parse_definition_status(value: &str) -> Result<AutomationDefinitionStatus, AutomationError> {
    AutomationDefinitionStatus::parse(value).ok_or_else(|| {
        AutomationError::InvalidStoredValue(format!("unknown definition status {value}"))
    })
}

fn parse_run_status(value: &str) -> Result<AutomationRunStatus, AutomationError> {
    AutomationRunStatus::parse(value)
        .ok_or_else(|| AutomationError::InvalidStoredValue(format!("unknown run status {value}")))
}

fn parse_step_kind(value: &str) -> Result<AutomationStepKind, AutomationError> {
    AutomationStepKind::parse(value)
        .ok_or_else(|| AutomationError::InvalidStoredValue(format!("unknown step kind {value}")))
}

fn parse_step_run_status(value: &str) -> Result<AutomationStepRunStatus, AutomationError> {
    AutomationStepRunStatus::parse(value).ok_or_else(|| {
        AutomationError::InvalidStoredValue(format!("unknown step run status {value}"))
    })
}

fn parse_job_status(value: &str) -> Result<AutomationJobStatus, AutomationError> {
    AutomationJobStatus::parse(value)
        .ok_or_else(|| AutomationError::InvalidStoredValue(format!("unknown job status {value}")))
}

fn parse_actor(kind: &str, id: &str) -> Result<Actor, AutomationError> {
    let kind = ActorKind::parse(kind)
        .ok_or_else(|| AutomationError::InvalidStoredValue(format!("unknown actor kind {kind}")))?;
    Actor::new(kind, id).map_err(|error| AutomationError::InvalidStoredValue(error.to_string()))
}

fn validate_project(project_id: ProjectId) -> Result<(), AutomationError> {
    if project_id.as_uuid().is_nil() {
        Err(AutomationValidationError::NilProjectId.into())
    } else {
        Ok(())
    }
}

fn job_lease_is_valid(row: &PgRow, worker_id: &str) -> bool {
    row.get::<String, _>("job_state") == "running"
        && row.get::<Option<String>, _>("lease_owner").as_deref() == Some(worker_id)
        && row
            .get::<Option<bool>, _>("lease_is_valid")
            .unwrap_or(false)
}

pub async fn begin_next_automation_step(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: AutomationRunId,
    worker_id: &str,
) -> Result<Option<AutomationStepRun>, AutomationError> {
    validate_project(project_id)?;
    validate_worker_id(worker_id)?;
    let mut transaction = pool.begin().await?;
    let run_and_job = sqlx::query(
        "SELECT r.status AS run_status, j.state AS job_state,
                j.lease_owner, j.leased_until, j.attempts AS job_attempts,
                (j.leased_until > now()) AS lease_is_valid
         FROM automation_runs AS r
         JOIN jobs AS j
           ON j.project_id = r.project_id AND j.id = r.job_id
         WHERE r.project_id=$1 AND r.id=$2
         FOR UPDATE OF r, j",
    )
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AutomationError::RunNotFound)?;
    let run_status = parse_run_status(&run_and_job.get::<String, _>("run_status"))?;
    match run_status {
        AutomationRunStatus::Completed => {
            transaction.commit().await?;
            return Ok(None);
        }
        AutomationRunStatus::Failed => return Err(AutomationError::RunNotRetryable),
        AutomationRunStatus::Queued => {
            if !job_lease_is_valid(&run_and_job, worker_id) {
                return Err(AutomationError::WorkerOwnership);
            }
            sqlx::query(
                "UPDATE automation_runs
                 SET status='running', started_at=COALESCE(started_at, now()),
                     finished_at=NULL, error=NULL
                 WHERE project_id=$1 AND id=$2",
            )
            .bind(project_id.as_uuid())
            .bind(run_id.as_uuid())
            .execute(&mut *transaction)
            .await?;
        }
        AutomationRunStatus::Running => {
            if !job_lease_is_valid(&run_and_job, worker_id) {
                return Err(AutomationError::WorkerOwnership);
            }
        }
    }

    let job_attempts: i32 = run_and_job.get("job_attempts");
    let row = sqlx::query(
        "SELECT id, project_id, automation_run_id, ordinal, step_key, step_kind,
                status, attempts, claimed_by, claimed_job_attempt,
                started_at, finished_at, error
         FROM automation_step_runs
         WHERE project_id=$1 AND automation_run_id=$2
           AND (
             status IN ('pending','failed')
             OR (status='running' AND claimed_job_attempt IS DISTINCT FROM $3)
           )
         ORDER BY ordinal
         LIMIT 1
         FOR UPDATE",
    )
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .bind(job_attempts)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(row) = row else {
        transaction.commit().await?;
        return Ok(None);
    };
    let old_status = parse_step_run_status(&row.get::<String, _>("status"))?;
    if old_status != AutomationStepRunStatus::Running {
        old_status
            .transition_to(AutomationStepRunStatus::Running)
            .map_err(|error| AutomationError::InvalidTransition(error.to_string()))?;
    }
    let updated = sqlx::query(
        "UPDATE automation_step_runs
         SET status='running', attempts=attempts+1, claimed_by=$4,
             claimed_job_attempt=$5, started_at=now(), finished_at=NULL, error=NULL
         WHERE project_id=$1 AND automation_run_id=$2 AND id=$3
           AND (
             status IN ('pending','failed')
             OR (status='running' AND claimed_job_attempt IS DISTINCT FROM $5)
           )
         RETURNING id, project_id, automation_run_id, ordinal, step_key, step_kind,
                   status, attempts, claimed_by, started_at, finished_at, error",
    )
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .bind(row.get::<Uuid, _>("id"))
    .bind(worker_id)
    .bind(job_attempts)
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(Some(step_run_from_row(&updated)?))
}

pub async fn complete_automation_step(
    pool: &PgPool,
    project_id: ProjectId,
    step_run_id: AutomationStepRunId,
    worker_id: &str,
) -> Result<bool, AutomationError> {
    complete_automation_step_with_output(pool, project_id, step_run_id, worker_id, None).await
}

pub async fn complete_automation_step_with_output(
    pool: &PgPool,
    project_id: ProjectId,
    step_run_id: AutomationStepRunId,
    worker_id: &str,
    output: Option<Value>,
) -> Result<bool, AutomationError> {
    validate_project(project_id)?;
    validate_worker_id(worker_id)?;
    let mut transaction = pool.begin().await?;
    let row = lock_step_and_job(&mut transaction, project_id, step_run_id).await?;
    let status = parse_step_run_status(&row.get::<String, _>("status"))?;
    if status == AutomationStepRunStatus::Completed {
        transaction.commit().await?;
        return Ok(false);
    }
    ensure_step_lease_owner(&row, status, worker_id)?;
    let job_attempts: i32 = row.get("job_attempts");
    let updated = sqlx::query(
        "UPDATE automation_step_runs
         SET status='completed', finished_at=now(), claimed_by=NULL,
             claimed_job_attempt=NULL, error=NULL, output=COALESCE($5, output)
         WHERE project_id=$1 AND id=$2 AND status='running' AND claimed_by=$3
           AND claimed_job_attempt=$4
         RETURNING id",
    )
    .bind(project_id.as_uuid())
    .bind(step_run_id.as_uuid())
    .bind(worker_id)
    .bind(job_attempts)
    .bind(output)
    .fetch_optional(&mut *transaction)
    .await?;
    if updated.is_none() {
        return Err(AutomationError::WorkerOwnership);
    }
    transaction.commit().await?;
    Ok(true)
}

pub async fn fail_automation_step(
    pool: &PgPool,
    project_id: ProjectId,
    step_run_id: AutomationStepRunId,
    worker_id: &str,
    error: &str,
) -> Result<bool, AutomationError> {
    validate_project(project_id)?;
    validate_worker_id(worker_id)?;
    validate_error(error)?;
    let mut transaction = pool.begin().await?;
    let row = lock_step_and_job(&mut transaction, project_id, step_run_id).await?;
    let status = parse_step_run_status(&row.get::<String, _>("status"))?;
    if status == AutomationStepRunStatus::Completed {
        transaction.commit().await?;
        return Ok(false);
    }
    ensure_step_lease_owner(&row, status, worker_id)?;
    let job_attempts: i32 = row.get("job_attempts");
    let updated = sqlx::query(
        "UPDATE automation_step_runs
         SET status='failed', finished_at=now(), claimed_by=NULL,
             claimed_job_attempt=NULL, error=$5
         WHERE project_id=$1 AND id=$2 AND status='running' AND claimed_by=$3
           AND claimed_job_attempt=$4
         RETURNING id",
    )
    .bind(project_id.as_uuid())
    .bind(step_run_id.as_uuid())
    .bind(worker_id)
    .bind(job_attempts)
    .bind(error)
    .fetch_optional(&mut *transaction)
    .await?;
    if updated.is_none() {
        return Err(AutomationError::WorkerOwnership);
    }
    transaction.commit().await?;
    Ok(true)
}

async fn lock_step_and_job(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
    step_run_id: AutomationStepRunId,
) -> Result<PgRow, AutomationError> {
    let row = sqlx::query(
        "SELECT s.status, s.claimed_by, s.claimed_job_attempt,
                j.state AS job_state, j.lease_owner, j.leased_until,
                j.attempts AS job_attempts,
                (j.leased_until > now()) AS lease_is_valid
         FROM automation_step_runs AS s
         JOIN automation_runs AS r
           ON r.project_id = s.project_id AND r.id = s.automation_run_id
         JOIN jobs AS j
           ON j.project_id = r.project_id AND j.id = r.job_id
         WHERE s.project_id=$1 AND s.id=$2
         FOR UPDATE OF s, j",
    )
    .bind(project_id.as_uuid())
    .bind(step_run_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AutomationError::StepNotFound)?;
    Ok(row)
}

fn ensure_step_lease_owner(
    row: &PgRow,
    status: AutomationStepRunStatus,
    worker_id: &str,
) -> Result<(), AutomationError> {
    if status != AutomationStepRunStatus::Running {
        return Err(AutomationError::InvalidTransition(format!(
            "step is {status:?}, not running"
        )));
    }
    if !job_lease_is_valid(row, worker_id)
        || row.get::<Option<String>, _>("claimed_by").as_deref() != Some(worker_id)
        || row.get::<Option<i32>, _>("claimed_job_attempt") != Some(row.get("job_attempts"))
    {
        return Err(AutomationError::WorkerOwnership);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomationFinalization {
    Completed,
    Failed,
}

pub async fn finalize_automation_run(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: AutomationRunId,
) -> Result<AutomationFinalization, AutomationError> {
    validate_project(project_id)?;
    let mut transaction = pool.begin().await?;
    let status = sqlx::query_scalar::<_, String>(
        "SELECT status
         FROM automation_runs
         WHERE project_id=$1 AND id=$2
         FOR UPDATE",
    )
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AutomationError::RunNotFound)?;
    let status = parse_run_status(&status)?;
    match status {
        AutomationRunStatus::Completed => {
            transaction.commit().await?;
            return Ok(AutomationFinalization::Completed);
        }
        AutomationRunStatus::Failed => {
            transaction.commit().await?;
            return Ok(AutomationFinalization::Failed);
        }
        AutomationRunStatus::Queued => return Err(AutomationError::RunNotReady),
        AutomationRunStatus::Running => {}
    }

    let summary = sqlx::query(
        "SELECT count(*)::bigint AS total,
                count(*) FILTER (WHERE status='completed')::bigint AS completed,
                count(*) FILTER (WHERE status='failed')::bigint AS failed,
                min(error) FILTER (WHERE status='failed') AS first_error
         FROM automation_step_runs
         WHERE project_id=$1 AND automation_run_id=$2",
    )
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .fetch_one(&mut *transaction)
    .await?;
    let total: i64 = summary.get("total");
    let completed: i64 = summary.get("completed");
    let failed: i64 = summary.get("failed");
    if total == 0 || (completed + failed) < total {
        return Err(AutomationError::RunNotReady);
    }
    if failed > 0 {
        let error = summary
            .get::<Option<String>, _>("first_error")
            .unwrap_or_else(|| "automation step failed".to_owned());
        validate_error(&error)?;
        sqlx::query(
            "UPDATE automation_runs
             SET status='failed', finished_at=now(), error=$3
             WHERE project_id=$1 AND id=$2",
        )
        .bind(project_id.as_uuid())
        .bind(run_id.as_uuid())
        .bind(error)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        return Ok(AutomationFinalization::Failed);
    }

    sqlx::query(
        "UPDATE automation_runs
         SET status='completed', finished_at=now(), error=NULL
         WHERE project_id=$1 AND id=$2",
    )
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(AutomationFinalization::Completed)
}

pub async fn retry_automation_run(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: AutomationRunId,
) -> Result<bool, AutomationError> {
    validate_project(project_id)?;
    let mut transaction = pool.begin().await?;
    let status = sqlx::query_scalar::<_, String>(
        "SELECT status FROM automation_runs
         WHERE project_id=$1 AND id=$2 FOR UPDATE",
    )
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AutomationError::RunNotFound)?;
    match parse_run_status(&status)? {
        AutomationRunStatus::Failed => {}
        AutomationRunStatus::Completed => {
            transaction.commit().await?;
            return Ok(false);
        }
        AutomationRunStatus::Queued | AutomationRunStatus::Running => {
            return Err(AutomationError::RunNotRetryable);
        }
    }
    sqlx::query(
        "UPDATE automation_runs
         SET status='queued', started_at=NULL, finished_at=NULL, error=NULL
         WHERE project_id=$1 AND id=$2",
    )
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "UPDATE jobs
         SET state='queued', available_at=now(), leased_until=NULL,
             lease_owner=NULL, lease_renewed_at=NULL, last_error=NULL, completed_at=NULL
         WHERE project_id=$1 AND id=(
           SELECT job_id FROM automation_runs WHERE project_id=$1 AND id=$2
         ) AND kind='automation_run'",
    )
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(true)
}

fn step_run_from_row(row: &PgRow) -> Result<AutomationStepRun, AutomationError> {
    Ok(AutomationStepRun {
        id: AutomationStepRunId::new(row.get("id"))?,
        project_id: ProjectId::new(row.get("project_id")),
        run_id: AutomationRunId::new(row.get("automation_run_id"))?,
        ordinal: row.get("ordinal"),
        key: row.get("step_key"),
        kind: parse_step_kind(&row.get::<String, _>("step_kind"))?,
        status: parse_step_run_status(&row.get::<String, _>("status"))?,
        attempts: row.get("attempts"),
        claimed_by: row.get("claimed_by"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
        error: row.get("error"),
    })
}
