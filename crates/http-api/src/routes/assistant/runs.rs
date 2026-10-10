//! Assistant durable runs: the run DTO, durable free-form submission, and
//! run status reads. Execution belongs to the worker; event observation lives
//! in [`super::run_events`].

use axum::{
    Json,
    extract::{Path, State},
};
use chrono::{DateTime, Utc};
use deepref_ai::ModelRouter;
use deepref_domain::Actor;
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

use super::{budget_exceeded_error, map_conversation_error, validate_project_id};
use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};
/// Assistant sampling temperature, shared by HTTP submission and the worker.
const ASSISTANT_TEMPERATURE: f32 = 0.2;

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AssistantRunDto {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub project_id: Uuid,
    pub trigger_message_id: Uuid,
    pub status: String,
    pub answer_message_id: Option<Uuid>,
    pub plan_id: Option<Uuid>,
    #[schema(value_type = Object)]
    pub error: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

fn run_dto(record: &deepref_postgres::AssistantAgentRunRecord) -> AssistantRunDto {
    AssistantRunDto {
        id: record.id,
        conversation_id: record.conversation_id,
        project_id: record.project_id,
        trigger_message_id: record.trigger_message_id,
        status: record.status.as_str().to_owned(),
        answer_message_id: record.answer_message_id,
        plan_id: record.plan_id,
        error: record.error.clone(),
        created_at: record.created_at,
        updated_at: record.updated_at,
        completed_at: record.completed_at,
    }
}

/// Durable free-form turn: persist the user message, the queued run and the
/// durable job in one transaction, then return the run identity. Execution
/// belongs to the worker; a browser disconnect cannot cancel it.
pub(super) async fn chat_durable(
    state: AppState,
    project_id: Uuid,
    actor: Actor,
    conversation_id: Uuid,
    user_message: String,
) -> Result<AssistantRunDto, ApiError> {
    if !state.ai_info.configured {
        return Err(ApiError::Configuration(
            "The assistant needs an AI provider, which is not configured for this workspace."
                .to_owned(),
        ));
    }
    let mut route = deepref_postgres::PostgresAiStore::new(&state.pool)
        .resolve(deepref_ai::ModelProfile::Reasoning)
        .await
        .map_err(|_| {
            ApiError::Configuration("No AI model is configured for the assistant.".to_owned())
        })?;
    route.parameters.temperature = Some(ASSISTANT_TEMPERATURE);
    let budget = deepref_postgres::get_ai_budget(&state.pool, project_id)
        .await
        .map_err(|error| match error {
            deepref_postgres::AiUsageError::ProjectNotFound => {
                ApiError::NotFound("project not found".to_owned())
            }
            _ => ApiError::Internal(anyhow::anyhow!("AI budget lookup failed")),
        })?;
    if budget.exhausted() {
        return Err(budget_exceeded_error());
    }
    let model_route = serde_json::to_value(&route)
        .map_err(|_| ApiError::Internal(anyhow::anyhow!("assistant route is not serializable")))?;
    let run = deepref_postgres::submit_assistant_agent_run(
        &state.pool,
        project_id,
        conversation_id,
        &user_message,
        actor.kind().as_str(),
        actor.id(),
        &model_route,
        deepref_ai::ASSISTANT_PROMPT_VERSION,
    )
    .await
    .map_err(map_conversation_error)?;
    Ok(run_dto(&run))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/assistant/runs/{run_id}",
    operation_id = "getAssistantRun",
    tag = "assistant",
    params(("project_id" = Uuid, Path), ("run_id" = Uuid, Path)),
    responses(
        (status = 200, description = "Assistant run", body = AssistantRunDto),
        (status = 404, description = "Run not found", body = ErrorResponse),
    )
)]
pub(crate) async fn get_run(
    State(state): State<AppState>,
    Path((project_id, run_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<AssistantRunDto>, ApiError> {
    validate_project_id(project_id)?;
    let record = deepref_postgres::get_assistant_agent_run(&state.pool, run_id)
        .await
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;
    match record.filter(|record| record.project_id == project_id) {
        Some(record) => Ok(Json(run_dto(&record))),
        None => Err(ApiError::NotFound("assistant run not found".to_owned())),
    }
}
