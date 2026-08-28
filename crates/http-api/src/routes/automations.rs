use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Utc};
use deepref_application::{
    AutomationDefinitionStatus, AutomationJobStatus, AutomationRunStatus, AutomationStepRunStatus,
    AutomationTriggerKind, BuiltInAutomationRecipe, ConfigureAutomationDefinition,
    StartAutomationManually, validate_run_list_limit,
};
use deepref_domain::ProjectId;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::review::extract_actor;
use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};

const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AutomationTriggerInput {
    ReportAdded,
    AcquisitionCompleted,
    FullTextAttached,
    ReportIncluded,
    StudyCreated,
    AppraisalCompleted,
    Manual,
}

impl From<AutomationTriggerInput> for AutomationTriggerKind {
    fn from(value: AutomationTriggerInput) -> Self {
        match value {
            AutomationTriggerInput::ReportAdded => Self::ReportAdded,
            AutomationTriggerInput::AcquisitionCompleted => Self::AcquisitionCompleted,
            AutomationTriggerInput::FullTextAttached => Self::FullTextAttached,
            AutomationTriggerInput::ReportIncluded => Self::ReportIncluded,
            AutomationTriggerInput::StudyCreated => Self::StudyCreated,
            AutomationTriggerInput::AppraisalCompleted => Self::AppraisalCompleted,
            AutomationTriggerInput::Manual => Self::Manual,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AutomationDefinitionStatusInput {
    Active,
    Paused,
}

impl From<AutomationDefinitionStatusInput> for AutomationDefinitionStatus {
    fn from(value: AutomationDefinitionStatusInput) -> Self {
        match value {
            AutomationDefinitionStatusInput::Active => Self::Active,
            AutomationDefinitionStatusInput::Paused => Self::Paused,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ConfigureAutomationRequest {
    pub name: String,
    pub trigger: AutomationTriggerInput,
    pub status: AutomationDefinitionStatusInput,
}

#[derive(Debug, Deserialize, IntoParams)]
pub(crate) struct AutomationRunListQuery {
    /// Maximum number of runs to return, from 1 through 100.
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct StartAutomationRequest {
    pub definition_id: Uuid,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct AutomationStepDto {
    pub ordinal: i32,
    pub key: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct AutomationDefinitionDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub recipe: String,
    pub version: i32,
    pub trigger: String,
    pub status: String,
    pub steps: Vec<AutomationStepDto>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct AutomationJobDto {
    pub id: Uuid,
    pub status: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub available_at: DateTime<Utc>,
    pub leased_until: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct AutomationStepRunDto {
    pub id: Uuid,
    pub ordinal: i32,
    pub key: String,
    pub kind: String,
    pub status: String,
    pub attempts: i32,
    pub claimed_by: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct AutomationUsageDto {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_micros: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct AutomationRunDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub definition_id: Uuid,
    pub recipe: String,
    pub version: i32,
    pub trigger: String,
    pub trigger_reference: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
    pub job: AutomationJobDto,
    pub steps: Vec<AutomationStepRunDto>,
    pub usage: AutomationUsageDto,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct StartAutomationResponse {
    pub run_id: Uuid,
    pub job_id: Uuid,
    pub created: bool,
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/automations/definitions",
    operation_id = "listAutomationDefinitions",
    tag = "automations",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    responses(
        (status = 200, description = "Configured automation definitions", body = Vec<AutomationDefinitionDto>),
        (status = 400, description = "Invalid project identifier", body = ErrorResponse),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_definitions(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<Vec<AutomationDefinitionDto>>, ApiError> {
    let project_id = validated_project_id(project_id)?;
    ensure_project(&state.pool, project_id).await?;
    let definitions = deepref_postgres::list_automation_definitions(&state.pool, project_id)
        .await
        .map_err(map_automation_error)?;
    Ok(Json(definitions.into_iter().map(definition_dto).collect()))
}

#[utoipa::path(
    put,
    path = "/projects/{project_id}/automations/definitions/{recipe}",
    operation_id = "configureAutomationDefinition",
    tag = "automations",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("recipe" = String, Path, description = "Closed built-in recipe identifier")
    ),
    request_body = ConfigureAutomationRequest,
    responses(
        (status = 200, description = "Configured automation definition", body = AutomationDefinitionDto),
        (status = 400, description = "Invalid automation configuration", body = ErrorResponse),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 409, description = "Automation configuration conflict", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn configure_definition(
    State(state): State<AppState>,
    Path((project_id, recipe)): Path<(Uuid, String)>,
    headers: HeaderMap,
    Json(input): Json<ConfigureAutomationRequest>,
) -> Result<Json<AutomationDefinitionDto>, ApiError> {
    let project_id = validated_project_id(project_id)?;
    ensure_project(&state.pool, project_id).await?;
    let recipe = BuiltInAutomationRecipe::parse(&recipe).ok_or_else(|| {
        ApiError::BadRequest("recipe must be a supported built-in automation recipe".to_owned())
    })?;
    let command = ConfigureAutomationDefinition::new(
        project_id,
        input.name,
        input.trigger.into(),
        recipe,
        input.status.into(),
        extract_actor(&headers)?,
    )
    .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let definition = deepref_postgres::configure_automation_definition(&state.pool, &command)
        .await
        .map_err(map_automation_error)?;
    Ok(Json(definition_dto(definition)))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/automations/runs",
    operation_id = "triggerAutomationManually",
    tag = "automations",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("Idempotency-Key" = String, Header, description = "Required stable key for replay-safe automation runs")
    ),
    request_body = StartAutomationRequest,
    responses(
        (status = 200, description = "Existing idempotent automation run", body = StartAutomationResponse),
        (status = 201, description = "Automation run queued", body = StartAutomationResponse),
        (status = 400, description = "Invalid automation request", body = ErrorResponse),
        (status = 404, description = "Project or definition not found", body = ErrorResponse),
        (status = 409, description = "Automation definition is paused or conflicting", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn trigger_manually(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<StartAutomationRequest>,
) -> Result<(StatusCode, Json<StartAutomationResponse>), ApiError> {
    let project_id = validated_project_id(project_id)?;
    ensure_project(&state.pool, project_id).await?;
    let request = StartAutomationManually::new(
        project_id,
        input.definition_id,
        required_idempotency_key(&headers)?,
        extract_actor(&headers)?,
    )
    .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let result = deepref_postgres::start_automation_manually(&state.pool, &request)
        .await
        .map_err(map_automation_error)?;
    let status = if result.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((
        status,
        Json(StartAutomationResponse {
            run_id: result.run_id.as_uuid(),
            job_id: result.job_id,
            created: result.created,
        }),
    ))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/automations/runs",
    operation_id = "listAutomationRuns",
    tag = "automations",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        AutomationRunListQuery
    ),
    responses(
        (status = 200, description = "Automation runs with job and usage visibility", body = Vec<AutomationRunDto>),
        (status = 400, description = "Invalid list limit", body = ErrorResponse),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_runs(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<AutomationRunListQuery>,
) -> Result<Json<Vec<AutomationRunDto>>, ApiError> {
    let project_id = validated_project_id(project_id)?;
    ensure_project(&state.pool, project_id).await?;
    let limit = query.limit.unwrap_or(25);
    validate_run_list_limit(limit).map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let runs = deepref_postgres::list_automation_runs(&state.pool, project_id, limit)
        .await
        .map_err(map_automation_error)?;
    Ok(Json(runs.into_iter().map(run_dto).collect()))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/automations/runs/{run_id}",
    operation_id = "getAutomationRun",
    tag = "automations",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("run_id" = Uuid, Path, description = "Automation run identifier")
    ),
    responses(
        (status = 200, description = "Automation run with job and usage visibility", body = AutomationRunDto),
        (status = 400, description = "Invalid identifier", body = ErrorResponse),
        (status = 404, description = "Project or run not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_run(
    State(state): State<AppState>,
    Path((project_id, run_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<AutomationRunDto>, ApiError> {
    let project_id = validated_project_id(project_id)?;
    ensure_project(&state.pool, project_id).await?;
    let run_id = deepref_application::AutomationRunId::new(run_id)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let run = deepref_postgres::get_automation_run(&state.pool, project_id, run_id)
        .await
        .map_err(map_automation_error)?;
    Ok(Json(run_dto(run)))
}

fn validated_project_id(value: Uuid) -> Result<ProjectId, ApiError> {
    if value.is_nil() {
        return Err(ApiError::BadRequest(
            "project_id must not be nil".to_owned(),
        ));
    }
    Ok(ProjectId::from(value))
}

async fn ensure_project(pool: &PgPool, project_id: ProjectId) -> Result<(), ApiError> {
    let exists = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
        .bind(project_id.as_uuid())
        .fetch_one(pool)
        .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::NotFound("project not found".to_owned()))
    }
}

fn required_idempotency_key(headers: &HeaderMap) -> Result<String, ApiError> {
    let value = headers
        .get(IDEMPOTENCY_KEY_HEADER)
        .ok_or_else(|| ApiError::BadRequest("Idempotency-Key header is required".to_owned()))?;
    let value = value
        .to_str()
        .map_err(|_| ApiError::BadRequest("Idempotency-Key must be valid ASCII".to_owned()))?
        .trim();
    if value.is_empty() || value.len() > 200 {
        return Err(ApiError::BadRequest(
            "Idempotency-Key must contain 1 through 200 characters".to_owned(),
        ));
    }
    Ok(value.to_owned())
}

fn definition_dto(
    definition: deepref_application::AutomationDefinition,
) -> AutomationDefinitionDto {
    AutomationDefinitionDto {
        id: definition.id.as_uuid(),
        project_id: definition.project_id.as_uuid(),
        name: definition.name.as_str().to_owned(),
        recipe: definition.recipe.id().to_owned(),
        version: definition.recipe.version(),
        trigger: definition.trigger.as_str().to_owned(),
        status: definition.status.as_str().to_owned(),
        steps: definition
            .steps
            .into_iter()
            .map(|step| AutomationStepDto {
                ordinal: step.ordinal,
                key: step.key,
                kind: step.kind.as_str().to_owned(),
            })
            .collect(),
        created_at: definition.created_at,
        updated_at: definition.updated_at,
    }
}

fn run_dto(run: deepref_application::AutomationRun) -> AutomationRunDto {
    AutomationRunDto {
        id: run.id.as_uuid(),
        project_id: run.project_id.as_uuid(),
        definition_id: run.definition_id.as_uuid(),
        recipe: run.recipe.id().to_owned(),
        version: run.recipe.version(),
        trigger: run.trigger.as_str().to_owned(),
        trigger_reference: run
            .trigger_reference
            .map(|reference| reference.as_str().to_owned()),
        status: run_status_name(run.status).to_owned(),
        created_at: run.created_at,
        started_at: run.started_at,
        finished_at: run.finished_at,
        error: run.error,
        job: AutomationJobDto {
            id: run.job.id,
            status: job_status_name(run.job.status).to_owned(),
            attempts: run.job.attempts,
            max_attempts: run.job.max_attempts,
            available_at: run.job.available_at,
            leased_until: run.job.leased_until,
            last_error: run.job.last_error,
        },
        steps: run
            .steps
            .into_iter()
            .map(|step| AutomationStepRunDto {
                id: step.id.as_uuid(),
                ordinal: step.ordinal,
                key: step.key,
                kind: step.kind.as_str().to_owned(),
                status: step_status_name(step.status).to_owned(),
                attempts: step.attempts,
                claimed_by: step.claimed_by,
                started_at: step.started_at,
                finished_at: step.finished_at,
                error: step.error,
            })
            .collect(),
        usage: AutomationUsageDto {
            input_tokens: run.usage.input_tokens,
            output_tokens: run.usage.output_tokens,
            cost_micros: run.usage.cost_micros,
        },
    }
}

fn run_status_name(status: AutomationRunStatus) -> &'static str {
    status.as_str()
}

fn step_status_name(status: AutomationStepRunStatus) -> &'static str {
    status.as_str()
}

fn job_status_name(status: AutomationJobStatus) -> &'static str {
    match status {
        AutomationJobStatus::Queued => "queued",
        AutomationJobStatus::Running => "running",
        AutomationJobStatus::Completed => "completed",
        AutomationJobStatus::Failed => "failed",
        AutomationJobStatus::Dead => "dead",
    }
}

fn map_automation_error(error: deepref_postgres::AutomationError) -> ApiError {
    match error {
        deepref_postgres::AutomationError::InvalidInput(error) => {
            ApiError::BadRequest(error.to_string())
        }
        deepref_postgres::AutomationError::DefinitionNotFound
        | deepref_postgres::AutomationError::RunNotFound => {
            ApiError::NotFound("automation resource not found".to_owned())
        }
        deepref_postgres::AutomationError::RunNotRetryable => ApiError::Conflict {
            code: "AUTOMATION_NOT_RETRYABLE".to_owned(),
            message: "automation run cannot be started from its current state".to_owned(),
            details: serde_json::json!({}),
        },
        deepref_postgres::AutomationError::InvalidTransition(message) => ApiError::Conflict {
            code: "AUTOMATION_STATE_CONFLICT".to_owned(),
            message: "automation state transition conflicted".to_owned(),
            details: serde_json::json!({ "reason": message }),
        },
        deepref_postgres::AutomationError::Serialization(error) => ApiError::Internal(error.into()),
        deepref_postgres::AutomationError::InvalidStoredValue(message) => {
            ApiError::DataIntegrity(message)
        }
        deepref_postgres::AutomationError::Database(error) => map_automation_database_error(error),
        deepref_postgres::AutomationError::StepNotFound
        | deepref_postgres::AutomationError::WorkerOwnership
        | deepref_postgres::AutomationError::RunNotReady
        | deepref_postgres::AutomationError::RunAlreadyTerminal => {
            ApiError::Internal(anyhow::anyhow!("unexpected automation worker state"))
        }
    }
}

fn map_automation_database_error(error: sqlx::Error) -> ApiError {
    let code = error
        .as_database_error()
        .and_then(|database_error| database_error.code());
    match code.as_deref() {
        Some("P0002") | Some("23503") => {
            ApiError::NotFound("automation resource not found".to_owned())
        }
        Some("55000") => ApiError::Conflict {
            code: "AUTOMATION_PAUSED".to_owned(),
            message: "automation definition is paused".to_owned(),
            details: serde_json::json!({}),
        },
        Some("22023") => ApiError::BadRequest("invalid automation request".to_owned()),
        _ => ApiError::Database(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_url_accepts_only_the_closed_built_in_catalog() {
        assert_eq!(
            BuiltInAutomationRecipe::parse("project_maintenance.v1"),
            Some(BuiltInAutomationRecipe::ProjectMaintenanceV1)
        );
        assert!(BuiltInAutomationRecipe::parse("arbitrary.v1").is_none());
    }

    #[test]
    fn job_status_mapping_is_closed() {
        assert_eq!(job_status_name(AutomationJobStatus::Dead), "dead");
    }
}
