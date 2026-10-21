use chrono::{DateTime, Utc};
use deepref_application::{
    AutomationDefinition, AutomationDefinitionId, AutomationDefinitionStatus,
    AutomationDomainEvent, AutomationIdempotencyKey, AutomationJobStatus, AutomationJobVisibility,
    AutomationName, AutomationRun, AutomationRunId, AutomationRunStatus, AutomationStepKind,
    AutomationStepRun, AutomationStepRunId, AutomationStepRunStatus, AutomationStepSnapshot,
    AutomationTriggerKind, AutomationTriggerReference, AutomationUsage, AutomationValidationError,
    BuiltInAutomationRecipe, ConfigureAutomationDefinition, DispatchAutomationTrigger,
    StartAutomationManually, jobs::JobLease, validate_error, validate_run_list_limit,
    validate_worker_id,
};
use deepref_domain::{Actor, ActorKind, ProjectId};
use serde::Deserialize;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use thiserror::Error;
use uuid::Uuid;

use crate::notifications::{NotificationDraft, record_notification_in_transaction};

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

const RUN_QUERY: &str = "SELECT r.id AS run_id,r.project_id,d.definition_id,r.job_id,
 d.recipe_id,d.recipe_version,d.trigger_kind,d.trigger_reference,d.idempotency_key,
 r.actor_kind,r.actor_id,r.status,r.created_at,r.started_at,r.finished_at,r.error AS run_error,
 j.state AS job_state,j.attempts AS job_attempts,j.max_attempts AS job_max_attempts,
 j.available_at AS job_available_at,j.leased_until AS job_leased_until,j.last_error AS job_last_error,
 COALESCE((SELECT jsonb_agg(jsonb_build_object('id',s.id,'ordinal',sd.ordinal,'key',sd.step_key,
 'kind',sd.step_kind,'status',CASE WHEN sd.step_key='execute_compiled_review' THEN
   CASE r.status WHEN 'queued' THEN 'pending' WHEN 'waiting' THEN 'running'
     WHEN 'blocked' THEN 'failed' WHEN 'cancelled' THEN 'failed' ELSE r.status END
   WHEN s.status='queued' THEN 'pending' ELSE s.status END,
 'attempts',s.attempts,'claimed_by',s.claimed_by,
 'started_at',CASE WHEN sd.step_key='execute_compiled_review' THEN r.started_at ELSE s.started_at END,
 'finished_at',CASE WHEN sd.step_key='execute_compiled_review' THEN r.finished_at ELSE s.finished_at END,
 'error',CASE WHEN sd.step_key='execute_compiled_review' THEN r.error ELSE s.error END) ORDER BY sd.ordinal)
 FROM durable_steps s JOIN automation_step_details sd ON sd.id=s.id
 WHERE s.project_id=r.project_id AND s.run_id=r.id),'[]'::jsonb) AS steps,
 COALESCE(u.input_tokens,0)::bigint AS input_tokens,COALESCE(u.output_tokens,0)::bigint AS output_tokens,
 COALESCE(u.cost_micros,0)::bigint AS cost_micros
 FROM durable_runs r JOIN automation_run_details d ON d.id=r.id AND d.project_id=r.project_id
 JOIN jobs j ON j.id=r.job_id AND j.project_id=r.project_id
 LEFT JOIN LATERAL (SELECT sum(input_tokens) input_tokens,sum(output_tokens) output_tokens,
 sum(cost_micros) cost_micros FROM ai_runs WHERE project_id=r.project_id AND parent_automation_run_id=r.id) u ON true";

pub(crate) async fn dispatch_automation_trigger_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    definition_id: Uuid,
    trigger: &str,
    reference: Option<&str>,
    idempotency_key: &str,
    actor: (ActorKind, &str),
) -> Result<(Uuid, Uuid, bool), sqlx::Error> {
    // Serialize the idempotency scope before checking the definition's current state.
    let identity = format!("{project_id}:{definition_id}:{trigger}:{idempotency_key}");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(&identity)
        .execute(&mut **transaction)
        .await?;
    if let Some(row) = sqlx::query("SELECT r.id,r.job_id FROM durable_runs r JOIN automation_run_details d ON d.id=r.id
        WHERE d.project_id=$1 AND d.definition_id=$2 AND d.trigger_kind=$3 AND d.idempotency_key=$4")
        .bind(project_id).bind(definition_id).bind(trigger).bind(idempotency_key)
        .fetch_optional(&mut **transaction).await? {
        return Ok((row.get("id"),row.get("job_id"),false));
    }
    let definition = sqlx::query(
        "SELECT recipe_id,recipe_version,status FROM automation_definitions
        WHERE project_id=$1 AND id=$2 AND trigger_kind=$3 FOR SHARE",
    )
    .bind(project_id)
    .bind(definition_id)
    .bind(trigger)
    .fetch_one(&mut **transaction)
    .await?;
    if definition.get::<String, _>("status") != "active" {
        // Keep the persisted dispatch contract: an existing paused definition
        // conflicts, while an absent or cross-project definition is not found.
        sqlx::query(
            "DO $$ BEGIN RAISE EXCEPTION 'automation definition is paused'
             USING ERRCODE = 'object_not_in_prerequisite_state'; END $$",
        )
        .execute(&mut **transaction)
        .await?;
    }
    let steps = sqlx::query(
        "SELECT ordinal,step_key,step_kind FROM automation_definition_steps
        WHERE project_id=$1 AND definition_id=$2 ORDER BY ordinal",
    )
    .bind(project_id)
    .bind(definition_id)
    .fetch_all(&mut **transaction)
    .await?;
    if steps.is_empty() {
        return Err(sqlx::Error::RowNotFound);
    }
    let run_id = Uuid::new_v4();
    let first_step_id = Uuid::new_v4();
    let job_id = enqueue_recipe_step(transaction, project_id, run_id, first_step_id).await?;
    sqlx::query("INSERT INTO durable_runs(id,project_id,kind,status,job_id,actor_kind,actor_id,idempotency_key)
        VALUES($1,$2,'automation','queued',$3,$4,$5,$6)")
        .bind(run_id).bind(project_id).bind(job_id).bind(actor.0.as_str()).bind(actor.1).bind(idempotency_key)
        .execute(&mut **transaction).await?;
    sqlx::query("INSERT INTO automation_run_details(id,project_id,definition_id,recipe_id,recipe_version,trigger_kind,trigger_reference,idempotency_key)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(run_id).bind(project_id).bind(definition_id).bind(definition.get::<String,_>("recipe_id"))
        .bind(definition.get::<i32,_>("recipe_version")).bind(trigger).bind(reference).bind(idempotency_key)
        .execute(&mut **transaction).await?;
    for (index, step) in steps.iter().enumerate() {
        let id = if index == 0 {
            first_step_id
        } else {
            Uuid::new_v4()
        };
        sqlx::query(
            "INSERT INTO durable_steps(id,project_id,run_id,handler_kind,step_key,ordinal,status)
            VALUES($1,$2,$3,'automation',$4,$5,'queued')",
        )
        .bind(id)
        .bind(project_id)
        .bind(run_id)
        .bind(step.get::<String, _>("step_key"))
        .bind(step.get::<i32, _>("ordinal"))
        .execute(&mut **transaction)
        .await?;
        sqlx::query(
            "INSERT INTO automation_step_details(id,project_id,run_id,ordinal,step_key,step_kind)
            VALUES($1,$2,$3,$4,$5,$6)",
        )
        .bind(id)
        .bind(project_id)
        .bind(run_id)
        .bind(step.get::<i32, _>("ordinal"))
        .bind(step.get::<String, _>("step_key"))
        .bind(step.get::<String, _>("step_kind"))
        .execute(&mut **transaction)
        .await?;
    }
    Ok((run_id, job_id, true))
}

async fn enqueue_recipe_step(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    run_id: Uuid,
    step_id: Uuid,
) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO jobs(id,project_id,kind,workload_class,payload,state,priority,max_attempts,dedupe_key)
        VALUES($1,$2,$3,$4,$5,'queued',0,5,$6)
        ON CONFLICT(dedupe_key) DO UPDATE SET id=jobs.id RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(crate::durable_runs::DURABLE_RUN_STEP_JOB_KIND)
    .bind(crate::durable_runs::DURABLE_RUN_STEP_WORKLOAD_CLASS)
    .bind(serde_json::json!({"run_id":run_id,"step_id":step_id,"handler_kind":"automation"}))
    .bind(format!("durable-step:{step_id}"))
    .fetch_one(&mut **transaction)
    .await
}

pub async fn dispatch_automation_trigger(
    pool: &PgPool,
    request: &DispatchAutomationTrigger,
) -> Result<AutomationDispatchResult, AutomationError> {
    request.validate()?;
    let mut transaction = pool.begin().await?;
    let (run_id, job_id, created) = dispatch_automation_trigger_in_transaction(
        &mut transaction,
        request.project_id.as_uuid(),
        request.definition_id.as_uuid(),
        request.trigger.as_str(),
        request
            .trigger_reference
            .as_ref()
            .map(AutomationTriggerReference::as_str),
        request.idempotency_key.as_str(),
        (request.actor.kind(), request.actor.id()),
    )
    .await?;
    transaction.commit().await?;
    Ok(AutomationDispatchResult {
        run_id: AutomationRunId::new(run_id)?,
        job_id,
        created,
    })
}

/// Dispatch a typed scientific transition inside the transaction that made
/// the transition authoritative. The automation definition snapshot and its
/// durable worker job therefore commit or roll back with the source command.
pub async fn dispatch_automation_domain_event(
    transaction: &mut Transaction<'_, Postgres>,
    event: &AutomationDomainEvent,
) -> Result<u64, sqlx::Error> {
    let project_id = event.project_id();
    let trigger = event.trigger();
    let source_identity = event.source_identity();
    let (source_actor_kind, source_actor_id) = event.actor();
    let (actor_kind, actor_id) = if source_actor_id.len() <= 200 {
        (source_actor_kind, source_actor_id)
    } else {
        (ActorKind::System, "automation-domain-event")
    };
    let mut created = crate::workflows::dispatch_workflow_event(transaction, event).await?;
    let Some(trigger) = trigger else {
        return Ok(created);
    };
    let definitions = sqlx::query_scalar::<_, Uuid>(
        "SELECT id
         FROM automation_definitions
         WHERE project_id=$1 AND trigger_kind=$2 AND status='active'
         ORDER BY id
         FOR SHARE",
    )
    .bind(project_id.as_uuid())
    .bind(trigger.as_str())
    .fetch_all(&mut **transaction)
    .await?;
    for definition_id in definitions {
        let (_, _, was_created) = dispatch_automation_trigger_in_transaction(
            transaction,
            project_id.as_uuid(),
            definition_id,
            trigger.as_str(),
            Some(&source_identity),
            &source_identity,
            (actor_kind, actor_id),
        )
        .await?;
        created += u64::from(was_created);
    }
    Ok(created)
}

pub async fn start_automation_manually(
    pool: &PgPool,
    request: &StartAutomationManually,
) -> Result<AutomationDispatchResult, AutomationError> {
    request.validate()?;
    let mut transaction = pool.begin().await?;
    let (run_id, job_id, created) = dispatch_automation_trigger_in_transaction(
        &mut transaction,
        request.project_id.as_uuid(),
        request.definition_id.as_uuid(),
        "manual",
        None,
        request.idempotency_key.as_str(),
        (request.actor.kind(), request.actor.id()),
    )
    .await?;
    transaction.commit().await?;
    Ok(AutomationDispatchResult {
        run_id: AutomationRunId::new(run_id)?,
        job_id,
        created,
    })
}

pub async fn list_automation_runs(
    pool: &PgPool,
    project_id: ProjectId,
    limit: i64,
) -> Result<Vec<AutomationRun>, AutomationError> {
    validate_project(project_id)?;
    validate_run_list_limit(limit)?;
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(RUN_QUERY);
    query.push(" WHERE r.project_id=$1 AND ai_first_run_visible($1,r.id) ORDER BY r.created_at DESC,r.id DESC LIMIT $2");
    let rows = query
        .build()
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
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(RUN_QUERY);
    query.push(" WHERE r.project_id=$1 AND r.id=$2 AND ai_first_run_visible($1,$2)");
    let row = query
        .build()
        .bind(project_id.as_uuid())
        .bind(run_id.as_uuid())
        .fetch_optional(pool)
        .await?
        .ok_or(AutomationError::RunNotFound)?;
    run_from_row(&row)
}

/// Worker-only recipe projection; a live run lease, not reviewer visibility,
/// authorizes execution of blinded review runs.
pub async fn get_leased_automation_run(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: AutomationRunId,
    lease: &JobLease,
) -> Result<AutomationRun, AutomationError> {
    validate_project(project_id)?;
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(RUN_QUERY);
    query.push(
        " WHERE r.project_id=$1 AND r.id=$2 AND EXISTS(
         SELECT 1 FROM jobs active WHERE active.id=$3 AND active.project_id=r.project_id
         AND active.kind='durable_run_step' AND active.payload->>'run_id'=r.id::text
         AND active.state='running' AND active.lease_owner=$4 AND active.claim_seq=$5
         AND active.leased_until>now())",
    );
    let row = query
        .build()
        .bind(project_id.as_uuid())
        .bind(run_id.as_uuid())
        .bind(lease.id)
        .bind(&lease.owner)
        .bind(lease.claim_seq)
        .fetch_optional(pool)
        .await?
        .ok_or(AutomationError::WorkerOwnership)?;
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
    let value = match value {
        "blocked" | "cancelled" => "failed",
        "waiting" => "running",
        value => value,
    };
    AutomationRunStatus::parse(value)
        .ok_or_else(|| AutomationError::InvalidStoredValue(format!("unknown run status {value}")))
}

fn parse_step_kind(value: &str) -> Result<AutomationStepKind, AutomationError> {
    AutomationStepKind::parse(value)
        .ok_or_else(|| AutomationError::InvalidStoredValue(format!("unknown step kind {value}")))
}

fn parse_step_run_status(value: &str) -> Result<AutomationStepRunStatus, AutomationError> {
    let value = match value {
        "queued" => "pending",
        "waiting" => "running",
        "blocked" | "cancelled" => "failed",
        "skipped" => "completed",
        value => value,
    };
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

async fn ensure_live_lease(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
    lease: &JobLease,
) -> Result<(), AutomationError> {
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM jobs WHERE id=$1 AND project_id=$2
        AND state='running' AND lease_owner=$3 AND claim_seq=$4 AND leased_until>now())",
    )
    .bind(lease.id)
    .bind(project_id.as_uuid())
    .bind(&lease.owner)
    .bind(lease.claim_seq)
    .fetch_one(&mut **transaction)
    .await?;
    if !valid {
        return Err(AutomationError::WorkerOwnership);
    }
    Ok(())
}

pub async fn begin_next_automation_step(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: AutomationRunId,
    lease: &JobLease,
) -> Result<Option<AutomationStepRun>, AutomationError> {
    validate_project(project_id)?;
    let worker_id = lease.owner.as_str();
    validate_worker_id(worker_id)?;
    let mut transaction = pool.begin().await?;
    ensure_live_lease(&mut transaction, project_id, lease).await?;
    let step_id = sqlx::query_scalar::<_,Uuid>(
        "SELECT s.id FROM durable_steps s JOIN jobs j ON j.id=$3 AND j.project_id=s.project_id
         WHERE s.project_id=$1 AND s.run_id=$2 AND j.kind='durable_run_step'
           AND j.payload->>'step_id'=s.id::text AND j.payload->>'run_id'=s.run_id::text
           AND NOT EXISTS(SELECT 1 FROM durable_steps p WHERE p.run_id=s.run_id AND p.ordinal<s.ordinal AND p.status<>'completed')",
    ).bind(project_id.as_uuid()).bind(run_id.as_uuid()).bind(lease.id)
        .fetch_optional(&mut *transaction).await?;
    let Some(step_id) = step_id else {
        return Ok(None);
    };
    if crate::durable_runs::begin_step(&mut transaction, step_id, lease)
        .await?
        .is_none()
    {
        transaction.commit().await?;
        return Ok(None);
    }
    let row = sqlx::query("SELECT s.*,d.step_kind FROM durable_steps s JOIN automation_step_details d ON d.id=s.id WHERE s.id=$1")
        .bind(step_id).fetch_one(&mut *transaction).await?;
    let step = step_run_from_row(&row)?;
    transaction.commit().await?;
    Ok(Some(step))
}

pub async fn complete_automation_step(
    pool: &PgPool,
    project_id: ProjectId,
    step_run_id: AutomationStepRunId,
    lease: &JobLease,
) -> Result<bool, AutomationError> {
    complete_automation_step_with_output(pool, project_id, step_run_id, lease, None).await
}

pub async fn complete_automation_step_with_output(
    pool: &PgPool,
    project_id: ProjectId,
    step_run_id: AutomationStepRunId,
    lease: &JobLease,
    output: Option<Value>,
) -> Result<bool, AutomationError> {
    validate_project(project_id)?;
    let worker_id = lease.owner.as_str();
    validate_worker_id(worker_id)?;
    let mut transaction = pool.begin().await?;
    let changed = complete_automation_step_with_output_in_transaction(
        &mut transaction,
        project_id,
        step_run_id,
        lease,
        output,
    )
    .await?;
    transaction.commit().await?;
    Ok(changed)
}

pub(crate) async fn complete_automation_step_with_output_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
    step_run_id: AutomationStepRunId,
    lease: &JobLease,
    output: Option<Value>,
) -> Result<bool, AutomationError> {
    let run_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT run_id FROM durable_steps WHERE project_id=$1 AND id=$2",
    )
    .bind(project_id.as_uuid())
    .bind(step_run_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await?;
    ensure_live_lease(transaction, project_id, lease).await?;
    let changed = crate::durable_runs::finish_step(
        transaction,
        step_run_id.as_uuid(),
        lease,
        "completed",
        None,
        output.as_ref(),
    )
    .await?;
    if !changed {
        let status =
            sqlx::query_scalar::<_, String>("SELECT status FROM durable_steps WHERE id=$1")
                .bind(step_run_id.as_uuid())
                .fetch_one(&mut **transaction)
                .await?;
        if status != "completed" {
            return Err(AutomationError::WorkerOwnership);
        }
        if let Some(output) = output.as_ref()
            && !crate::durable_runs::set_completed_step_output(
                transaction,
                step_run_id.as_uuid(),
                lease,
                output,
            )
            .await?
        {
            return Err(AutomationError::WorkerOwnership);
        }
    }
    if changed {
        let next = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM durable_steps WHERE project_id=$1 AND run_id=$2
            AND status='queued' ORDER BY ordinal LIMIT 1",
        )
        .bind(project_id.as_uuid())
        .bind(run_id)
        .fetch_optional(&mut **transaction)
        .await?;
        if let Some(next) = next {
            enqueue_recipe_step(transaction, project_id.as_uuid(), run_id, next).await?;
        }
    }
    Ok(changed)
}

pub async fn fail_automation_step(
    pool: &PgPool,
    project_id: ProjectId,
    step_run_id: AutomationStepRunId,
    lease: &JobLease,
    error: &str,
) -> Result<bool, AutomationError> {
    validate_project(project_id)?;
    let worker_id = lease.owner.as_str();
    validate_worker_id(worker_id)?;
    validate_error(error)?;
    let mut transaction = pool.begin().await?;
    ensure_live_lease(&mut transaction, project_id, lease).await?;
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM durable_steps WHERE project_id=$1 AND id=$2)",
    )
    .bind(project_id.as_uuid())
    .bind(step_run_id.as_uuid())
    .fetch_one(&mut *transaction)
    .await?;
    if !exists {
        return Err(AutomationError::StepNotFound);
    }
    let changed = crate::durable_runs::finish_step(
        &mut transaction,
        step_run_id.as_uuid(),
        lease,
        "failed",
        Some(error),
        None,
    )
    .await?;
    if !changed {
        let status =
            sqlx::query_scalar::<_, String>("SELECT status FROM durable_steps WHERE id=$1")
                .bind(step_run_id.as_uuid())
                .fetch_one(&mut *transaction)
                .await?;
        if status != "completed" {
            return Err(AutomationError::WorkerOwnership);
        }
    }
    transaction.commit().await?;
    Ok(changed)
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
    lease: &JobLease,
) -> Result<AutomationFinalization, AutomationError> {
    validate_project(project_id)?;
    let mut transaction = pool.begin().await?;
    let status = sqlx::query_scalar::<_, String>(
        "SELECT status
         FROM durable_runs
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
         FROM durable_steps
         WHERE project_id=$1 AND run_id=$2",
    )
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .fetch_one(&mut *transaction)
    .await?;
    let total: i64 = summary.get("total");
    let completed: i64 = summary.get("completed");
    let failed: i64 = summary.get("failed");
    if total == 0 || (failed == 0 && completed < total) {
        return Err(AutomationError::RunNotReady);
    }
    let current = sqlx::query_scalar::<_, Uuid>(
        "SELECT s.id FROM jobs j JOIN durable_steps s ON s.project_id=j.project_id
         AND j.payload->>'step_id'=s.id::text AND j.payload->>'run_id'=s.run_id::text
         WHERE j.id=$1 AND s.project_id=$2 AND s.run_id=$3",
    )
    .bind(lease.id)
    .bind(project_id.as_uuid())
    .bind(run_id.as_uuid())
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AutomationError::WorkerOwnership)?;
    if !crate::durable_runs::verify_active_step_lease(&mut transaction, current, lease).await? {
        return Err(AutomationError::WorkerOwnership);
    }
    if failed > 0 {
        let error = summary
            .get::<Option<String>, _>("first_error")
            .unwrap_or_else(|| "automation step failed".to_owned());
        validate_error(&error)?;
        crate::durable_runs::finish_run(&mut transaction, run_id.as_uuid(), "failed", Some(&error))
            .await?;
        // A review run that failed or was blocked has already notified once,
        // with the review's own name and reason (see `review_runs`). A second
        // "Automation failed" for the same run only duplicates that entry.
        let review_already_reported: bool = sqlx::query_scalar(
            "SELECT EXISTS (
               SELECT 1 FROM review_run_manifests m
               JOIN durable_runs r ON r.id=m.automation_run_id
               WHERE m.project_id=$1 AND m.automation_run_id=$2
                 AND r.status IN ('failed','blocked')
             )",
        )
        .bind(project_id.as_uuid())
        .bind(run_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await?;
        if !review_already_reported {
            record_notification_in_transaction(
                &mut transaction,
                &NotificationDraft::error(
                    "automation_run.failed",
                    Some(project_id.as_uuid()),
                    "Automation failed",
                    Some(error),
                    serde_json::json!({ "run_id": run_id.as_uuid() }),
                ),
            )
            .await?;
        }
        transaction.commit().await?;
        return Ok(AutomationFinalization::Failed);
    }

    crate::durable_runs::finish_run(&mut transaction, run_id.as_uuid(), "completed", None).await?;
    record_notification_in_transaction(
        &mut transaction,
        &NotificationDraft::success(
            "automation_run.completed",
            Some(project_id.as_uuid()),
            "Automation completed",
            None,
            serde_json::json!({ "run_id": run_id.as_uuid() }),
        ),
    )
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
        "SELECT status FROM durable_runs
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
    if !crate::durable_runs::reopen_recipe_run(&mut transaction, run_id.as_uuid()).await? {
        return Err(AutomationError::RunNotRetryable);
    }
    // Requeue the failed step's existing job; the run's first job identity never changes.
    sqlx::query(
        "UPDATE jobs
         SET state='queued', available_at=now(), leased_until=NULL,
             lease_owner=NULL, lease_renewed_at=NULL, last_error=NULL, completed_at=NULL,
             max_attempts=GREATEST(max_attempts, attempts + 1)
         WHERE project_id=$1 AND payload->>'step_id'=(
           SELECT id::text FROM durable_steps WHERE project_id=$1 AND run_id=$2 AND status='queued' ORDER BY ordinal LIMIT 1
         ) AND kind='durable_run_step'",
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
        run_id: AutomationRunId::new(row.get("run_id"))?,
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
