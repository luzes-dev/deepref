//! Assistant HTTP surface: the sync tool-command path ([`chat`]), durable
//! runs ([`runs`]), run event observation ([`run_events`]), and plans
//! ([`plans`]). This root holds the shared kernel (validation, failure
//! mapping, tool-execution core, SSE framing); endpoints live in the child
//! modules and are re-exported here so existing `assistant::` paths keep
//! working.

use axum::response::sse::Event;
use deepref_ai::{AgentProposalOperation, AgentToolError, AssistantStreamEvent};
use deepref_domain::{Actor, ScreeningStage};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{error::ApiError, state::AppState};

mod chat;
mod plans;
mod run_events;
mod runs;

pub(crate) use chat::*;
pub(crate) use plans::*;
pub(crate) use run_events::*;
pub(crate) use runs::*;
#[derive(Debug, Clone, Copy)]
enum AssistantFailure {
    NotFound,
    Conflict,
    Internal,
}

async fn execute_proposal(
    state: &AppState,
    operation: AgentProposalOperation,
    actor: Actor,
) -> Result<deepref_review::ReviewRunSnapshot, AssistantFailure> {
    let result = match operation {
        AgentProposalOperation::ProposeScreeningDecision(args) => {
            deepref_postgres::schedule_screening_review(
                &state.pool,
                args.project_id.as_uuid(),
                args.report_id.as_uuid(),
                match args.stage {
                    ScreeningStage::TitleAbstract => deepref_ai::ScreeningStage::TitleAbstract,
                    ScreeningStage::FullText => deepref_ai::ScreeningStage::FullText,
                },
                None,
                None,
                actor,
            )
            .await
        }
        AgentProposalOperation::ProposeDuplicateMerge(args) => {
            deepref_postgres::schedule_duplicate_detection_review(
                &state.pool,
                args.project_id.as_uuid(),
                args.source_record_id.as_uuid(),
                args.candidate_report_id.as_uuid(),
                actor,
            )
            .await
        }
        AgentProposalOperation::ProposeStudyGrouping(args) => {
            deepref_postgres::schedule_study_grouping_review(
                &state.pool,
                args.project_id.as_uuid(),
                args.report_id.as_uuid(),
                actor,
            )
            .await
        }
        AgentProposalOperation::ProposeClassification(args) => {
            deepref_postgres::schedule_study_classification_review(
                &state.pool,
                args.project_id.as_uuid(),
                args.study_id.as_uuid(),
                actor,
            )
            .await
        }
        AgentProposalOperation::ProposeExtraction(args) => {
            deepref_postgres::schedule_data_extraction_review(
                &state.pool,
                args.project_id.as_uuid(),
                args.study_id.as_uuid(),
                actor,
            )
            .await
        }
        AgentProposalOperation::ProposeAppraisalAnswer(args) => {
            deepref_postgres::schedule_appraisal_prefill_review(
                &state.pool,
                args.project_id.as_uuid(),
                args.report_id.as_uuid(),
                &args.definition_id,
                args.definition_version,
                actor,
            )
            .await
        }
    };
    result.map_err(review_preparation_failure)
}

fn review_preparation_failure(error: deepref_postgres::ReviewPreparationError) -> AssistantFailure {
    match error {
        deepref_postgres::ReviewPreparationError::Protocol(
            deepref_postgres::ProtocolError::ProjectNotFound
            | deepref_postgres::ProtocolError::NotFound,
        )
        | deepref_postgres::ReviewPreparationError::Study(
            deepref_postgres::StudyError::ProjectNotFound
            | deepref_postgres::StudyError::StudyNotFound
            | deepref_postgres::StudyError::ReportNotInProject,
        ) => AssistantFailure::NotFound,
        deepref_postgres::ReviewPreparationError::InvalidInput(_) => AssistantFailure::Conflict,
        _ => AssistantFailure::Internal,
    }
}

fn map_conversation_error(error: deepref_postgres::AssistantError) -> ApiError {
    match error {
        deepref_postgres::AssistantError::ConversationNotFound => {
            ApiError::NotFound("assistant conversation not found".to_owned())
        }
        deepref_postgres::AssistantError::ProjectNotFound => {
            ApiError::NotFound("project not found".to_owned())
        }
        deepref_postgres::AssistantError::InvalidInput(message) => ApiError::BadRequest(message),
        deepref_postgres::AssistantError::Database(error) => {
            ApiError::Internal(anyhow::anyhow!(error))
        }
    }
}

pub(super) fn budget_exceeded_error() -> ApiError {
    ApiError::Conflict {
        code: "ai_budget_exceeded".to_owned(),
        message: "AI budget for this month reached".to_owned(),
        details: Value::Null,
    }
}

fn subscription_limit_error() -> ApiError {
    ApiError::Conflict {
        code: "ai_subscription_limit".to_owned(),
        message: "AI subscription limit reached; try again later".to_owned(),
        details: Value::Null,
    }
}

fn provider_failed_error() -> ApiError {
    ApiError::Configuration("The AI provider could not be reached. Try again shortly.".to_owned())
}

fn validate_project_id(project_id: Uuid) -> Result<(), ApiError> {
    if project_id.is_nil() {
        Err(ApiError::BadRequest(
            "project_id must not be nil".to_owned(),
        ))
    } else {
        Ok(())
    }
}

fn sse_frame(name: &str, payload: &Value) -> Result<Event, std::convert::Infallible> {
    Ok(Event::default().event(name).data(payload.to_string()))
}

fn stream_event_parts(event: &AssistantStreamEvent) -> (&'static str, Value) {
    match event {
        AssistantStreamEvent::Token { delta } => ("token", json!({ "delta": delta })),
        AssistantStreamEvent::Replace { text } => ("replace", json!({ "text": text })),
        AssistantStreamEvent::ToolStart {
            tool,
            tool_call_id,
            args,
        } => (
            "tool_start",
            json!({ "tool": tool, "tool_call_id": tool_call_id, "args": args }),
        ),
        AssistantStreamEvent::ToolComplete {
            tool,
            tool_call_id,
            output,
        } => (
            "tool_complete",
            json!({ "tool": tool, "tool_call_id": tool_call_id, "output": output }),
        ),
        AssistantStreamEvent::ProposalCreated {
            tool,
            review_run_id,
            status_path,
        } => (
            "proposal_created",
            json!({
                "tool": tool,
                "review_run_id": review_run_id,
                "status_path": status_path,
            }),
        ),
        AssistantStreamEvent::Status { message } => ("status", json!({ "message": message })),
        AssistantStreamEvent::PlanProposed { plan } => ("plan", json!({ "plan": plan })),
        AssistantStreamEvent::Done {
            message_id,
            input_tokens,
            output_tokens,
        } => (
            "done",
            json!({
                "message_id": message_id,
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
            }),
        ),
    }
}

fn stream_event_frame(event: &AssistantStreamEvent) -> Result<Event, std::convert::Infallible> {
    let (name, payload) = stream_event_parts(event);
    sse_frame(name, &payload)
}

fn assistant_error_frame(error: AgentToolError) -> Result<Event, std::convert::Infallible> {
    let (code, message) = match error {
        AgentToolError::BudgetExceeded => {
            ("ai_budget_exceeded", "AI budget for this month reached")
        }
        AgentToolError::SubscriptionLimit => (
            "ai_subscription_limit",
            "AI subscription limit reached; try again later",
        ),
        AgentToolError::ProviderFailed => (
            "ai_provider_failed",
            "The AI provider could not be reached. Try again shortly.",
        ),
        AgentToolError::NotConfigured => (
            "ai_not_configured",
            "The assistant needs an AI provider, which is not configured for this workspace.",
        ),
        _ => ("assistant_failed", "assistant turn failed"),
    };
    sse_frame("error", &json!({ "code": code, "message": message }))
}
