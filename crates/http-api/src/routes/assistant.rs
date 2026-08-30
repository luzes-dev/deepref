use std::sync::{Arc, Mutex};

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use deepref_ai::{
    AgentDispatch, AgentProposalOperation, AgentProposalReceipt, AgentReadOperation, AgentRuntime,
    AgentTool, AgentToolError, AgentToolExecutionError, AgentToolExecutor, AgentToolName,
    AgentToolParseError, BoundedAgentJson,
};
use deepref_domain::{Actor, ProjectId, ScreeningStage, StudyDesign};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;
use uuid::Uuid;

use super::review::extract_actor;
use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};
const MAX_BLOCK_TEXT_CHARS: usize = 2_000;
const MAX_REPORT_ABSTRACT_CHARS: usize = 4_000;

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AssistantToolKind {
    Read,
    Proposal,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct AssistantToolDescriptor {
    pub name: String,
    pub kind: AssistantToolKind,
    pub authority_tier: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AssistantToolNameDto {
    GetProjectProtocol,
    GetReport,
    ReadDocumentBlocks,
    SearchDocument,
    SearchProjectReports,
    GetScreeningState,
    GetStudy,
    GetAppraisal,
    ProposeScreeningDecision,
    ProposeDuplicateMerge,
    ProposeStudyGrouping,
    ProposeClassification,
    ProposeExtraction,
    ProposeAppraisalAnswer,
}

#[derive(Debug, Deserialize, ToSchema)]
#[allow(dead_code)]
pub(crate) struct AssistantToolRequest {
    /// The value is parsed by deepref_ai::AgentTool, which owns the closed
    /// name and typed-argument schema.
    pub tool: AssistantToolNameDto,
    #[schema(value_type = Object)]
    pub args: Value,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AssistantToolResponse {
    Read {
        #[schema(value_type = Object)]
        data: Value,
    },
    ReviewRun {
        review_run_id: Uuid,
        status_path: String,
    },
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/assistant/tools",
    operation_id = "listProjectAssistantTools",
    tag = "assistant",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    responses(
        (status = 200, description = "The closed project-assistant tool catalog", body = Vec<AssistantToolDescriptor>),
        (status = 400, description = "Invalid project identifier", body = ErrorResponse),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_tools(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<Vec<AssistantToolDescriptor>>, ApiError> {
    validate_project_id(project_id)?;
    if !deepref_postgres::project_exists(&state.pool, project_id).await? {
        return Err(ApiError::NotFound("project not found".to_owned()));
    }
    Ok(Json(
        AgentToolName::ALL
            .into_iter()
            .map(tool_descriptor)
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/assistant/tools/execute",
    operation_id = "executeProjectAssistantTool",
    tag = "assistant",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    request_body = AssistantToolRequest,
    responses(
        (status = 200, description = "Bounded read result or scheduled review run", body = AssistantToolResponse),
        (status = 400, description = "Malformed or invalid tool request", body = ErrorResponse),
        (status = 403, description = "Tool is forbidden by the project policy", body = ErrorResponse),
        (status = 404, description = "Scoped project or resource was not found", body = ErrorResponse),
        (status = 409, description = "Proposal conflicts with current state", body = ErrorResponse),
        (status = 503, description = "AI provider is unavailable", body = ErrorResponse),
        (status = 500, description = "Opaque tool execution failure", body = ErrorResponse)
    )
)]
pub(crate) async fn execute_tool(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<AssistantToolResponse>, ApiError> {
    validate_project_id(project_id)?;
    let actor = extract_actor(&headers)?;
    if !deepref_postgres::project_exists(&state.pool, project_id).await? {
        return Err(ApiError::NotFound("project not found".to_owned()));
    }
    let encoded = serde_json::to_string(&body)
        .map_err(|_| ApiError::BadRequest("tool request is malformed".to_owned()))?;
    let tool = AgentTool::parse_json(&encoded).map_err(map_parse_error)?;
    let runtime = AgentRuntime::new(
        ProjectId::new(project_id),
        deepref_ai::ProjectAiPolicy::default(),
    );
    let executor = ProjectAgentToolExecutor::new(state, actor.clone());
    let dispatch = runtime
        .dispatch(&actor, tool, &executor)
        .map_err(map_runtime_error)?;
    let response = match dispatch {
        AgentDispatch::Read(future) => match future.await {
            Ok(data) => AssistantToolResponse::Read {
                data: data.into_value(),
            },
            Err(_) => return Err(executor.take_failure()),
        },
        AgentDispatch::Proposal(future) => match future.await {
            Ok(AgentProposalReceipt { review_run_id }) => AssistantToolResponse::ReviewRun {
                review_run_id,
                status_path: format!("/projects/{project_id}/review-runs/{review_run_id}"),
            },
            Err(_) => return Err(executor.take_failure()),
        },
    };
    Ok(Json(response))
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

fn tool_descriptor(name: AgentToolName) -> AssistantToolDescriptor {
    AssistantToolDescriptor {
        name: name.as_str().to_owned(),
        kind: if name.is_read() {
            AssistantToolKind::Read
        } else {
            AssistantToolKind::Proposal
        },
        authority_tier: name.policy().authority.as_str().to_owned(),
        description: tool_description(name).to_owned(),
    }
}

fn tool_description(name: AgentToolName) -> &'static str {
    match name {
        AgentToolName::GetProjectProtocol => "Read the published project protocol.",
        AgentToolName::GetReport => "Read one report scoped to the project.",
        AgentToolName::ReadDocumentBlocks => "Read selected active document blocks.",
        AgentToolName::SearchDocument => "Search active blocks in one document.",
        AgentToolName::SearchProjectReports => "Search project reports by metadata.",
        AgentToolName::GetScreeningState => "Read the screening state for one report.",
        AgentToolName::GetStudy => "Read one study and its report membership.",
        AgentToolName::GetAppraisal => "Read the latest completed appraisal version.",
        AgentToolName::ProposeScreeningDecision => "Generate a reviewer proposal for screening.",
        AgentToolName::ProposeDuplicateMerge => {
            "Generate a reviewer proposal for a duplicate pair."
        }
        AgentToolName::ProposeStudyGrouping => "Generate a reviewer proposal for report grouping.",
        AgentToolName::ProposeClassification => "Generate a reviewer proposal for study design.",
        AgentToolName::ProposeExtraction => "Generate a reviewer proposal for data extraction.",
        AgentToolName::ProposeAppraisalAnswer => {
            "Generate a reviewer proposal for appraisal answers."
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum AssistantFailure {
    NotFound,
    Conflict,
    Internal,
}

struct ProjectAgentToolExecutor {
    state: AppState,
    actor: Actor,
    failure: Arc<Mutex<Option<AssistantFailure>>>,
}

impl ProjectAgentToolExecutor {
    fn new(state: AppState, actor: Actor) -> Self {
        Self {
            state,
            actor,
            failure: Arc::new(Mutex::new(None)),
        }
    }

    fn record_failure(
        failure: &Mutex<Option<AssistantFailure>>,
        kind: AssistantFailure,
    ) -> AgentToolExecutionError {
        if let Ok(mut recorded) = failure.lock() {
            *recorded = Some(kind);
        }
        AgentToolExecutionError
    }

    fn take_failure(&self) -> ApiError {
        let kind = self
            .failure
            .lock()
            .ok()
            .and_then(|mut failure| failure.take())
            .unwrap_or(AssistantFailure::Internal);
        match kind {
            AssistantFailure::NotFound => {
                ApiError::NotFound("scoped resource not found".to_owned())
            }
            AssistantFailure::Conflict => ApiError::Conflict {
                code: "assistant_proposal_conflict".to_owned(),
                message: "proposal conflicts with current state".to_owned(),
                details: Value::Null,
            },
            AssistantFailure::Internal => {
                ApiError::Internal(anyhow::anyhow!("assistant tool execution failed"))
            }
        }
    }
}

impl AgentToolExecutor for ProjectAgentToolExecutor {
    fn execute_read<'a>(
        &'a self,
        operation: AgentReadOperation,
    ) -> deepref_ai::AgentReadFuture<'a> {
        let state = self.state.clone();
        let failure = Arc::clone(&self.failure);
        Box::pin(async move {
            match execute_read(&state, operation).await {
                Ok(value) => BoundedAgentJson::new(value)
                    .map_err(|_| Self::record_failure(&failure, AssistantFailure::Internal)),
                Err(kind) => Err(Self::record_failure(&failure, kind)),
            }
        })
    }

    fn create_proposal<'a>(
        &'a self,
        operation: AgentProposalOperation,
    ) -> deepref_ai::AgentProposalFuture<'a> {
        let state = self.state.clone();
        let actor = self.actor.clone();
        let failure = Arc::clone(&self.failure);
        Box::pin(async move {
            match execute_proposal(&state, operation, actor).await {
                Ok(run) => Ok(AgentProposalReceipt {
                    review_run_id: run.id.as_uuid(),
                }),
                Err(kind) => Err(Self::record_failure(&failure, kind)),
            }
        })
    }
}

async fn execute_read(
    state: &AppState,
    operation: AgentReadOperation,
) -> Result<Value, AssistantFailure> {
    match operation {
        AgentReadOperation::GetProjectProtocol(args) => {
            let protocol =
                deepref_postgres::get_published_protocol(&state.pool, args.project_id.as_uuid())
                    .await
                    .map_err(protocol_failure)?;
            protocol_value(protocol)
        }
        AgentReadOperation::GetReport(args) => {
            let report = deepref_postgres::get_agent_report(
                &state.pool,
                args.project_id.as_uuid(),
                args.report_id.as_uuid(),
            )
            .await
            .map_err(read_failure)?;
            Ok(report_value(report, MAX_REPORT_ABSTRACT_CHARS))
        }
        AgentReadOperation::ReadDocumentBlocks(args) => {
            let block_ids = args
                .block_ids
                .iter()
                .map(|block_id| block_id.as_uuid())
                .collect::<Vec<_>>();
            let blocks = deepref_postgres::read_agent_document_blocks(
                &state.pool,
                args.project_id.as_uuid(),
                args.document_id.as_uuid(),
                &block_ids,
            )
            .await
            .map_err(read_failure)?;
            Ok(blocks_value(blocks))
        }
        AgentReadOperation::SearchDocument(args) => {
            let blocks = deepref_postgres::search_agent_document(
                &state.pool,
                args.project_id.as_uuid(),
                args.document_id.as_uuid(),
                &args.query,
                i64::from(args.limit),
            )
            .await
            .map_err(read_failure)?;
            Ok(blocks_value(blocks))
        }
        AgentReadOperation::SearchProjectReports(args) => {
            let reports = deepref_postgres::search_agent_reports(
                &state.pool,
                args.project_id.as_uuid(),
                &args.query,
                i64::from(args.limit),
            )
            .await
            .map_err(read_failure)?;
            Ok(Value::Array(
                reports
                    .into_iter()
                    .map(|report| report_value(report, MAX_REPORT_ABSTRACT_CHARS))
                    .collect(),
            ))
        }
        AgentReadOperation::GetScreeningState(args) => {
            let screening = deepref_postgres::get_agent_screening_state(
                &state.pool,
                args.project_id.as_uuid(),
                args.report_id.as_uuid(),
            )
            .await
            .map_err(read_failure)?;
            serde_json::to_value(screening).map_err(|_| AssistantFailure::Internal)
        }
        AgentReadOperation::GetStudy(args) => {
            let study = deepref_postgres::get_study(
                &state.pool,
                args.project_id.as_uuid(),
                args.study_id.as_uuid(),
            )
            .await
            .map_err(study_failure)?;
            study_value(study)
        }
        AgentReadOperation::GetAppraisal(args) => {
            let appraisal = deepref_postgres::get_latest_agent_appraisal(
                &state.pool,
                args.project_id.as_uuid(),
                args.report_id.as_uuid(),
                &args.definition_id,
                i32::try_from(args.definition_version).map_err(|_| AssistantFailure::Internal)?,
            )
            .await
            .map_err(read_failure)?;
            serde_json::to_value(appraisal).map_err(|_| AssistantFailure::Internal)
        }
    }
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

fn protocol_value(protocol: deepref_postgres::ProtocolDocument) -> Result<Value, AssistantFailure> {
    let framework =
        serde_json::to_value(protocol.framework).map_err(|_| AssistantFailure::Internal)?;
    let criteria =
        serde_json::to_value(protocol.criteria).map_err(|_| AssistantFailure::Internal)?;
    Ok(json!({
        "id": protocol.id,
        "project_id": protocol.project_id,
        "version": protocol.version,
        "name": protocol.name,
        "status": protocol.status,
        "framework": framework,
        "objective": protocol.objective,
        "question": protocol.question,
        "criteria": criteria,
        "revision": protocol.revision,
        "published_at": protocol.published_at,
    }))
}

fn report_value(report: deepref_postgres::AgentReportRecord, abstract_limit: usize) -> Value {
    json!({
        "id": report.id,
        "project_id": report.project_id,
        "title": report.title,
        "abstract_text": report.abstract_text.as_deref().map(|value| bounded_text(value, abstract_limit)),
        "publication_year": report.publication_year,
        "journal": report.journal,
        "url": report.url,
        "identifiers": report.identifiers,
    })
}

fn blocks_value(blocks: Vec<deepref_postgres::AgentDocumentBlockRecord>) -> Value {
    Value::Array(
        blocks
            .into_iter()
            .map(|block| {
                json!({
                    "id": block.id,
                    "document_id": block.document_id,
                    "page_number": block.page_number,
                    "kind": block.kind,
                    "section_path": block.section_path,
                    "ordinal": block.ordinal,
                    "text": bounded_text(&block.text, MAX_BLOCK_TEXT_CHARS),
                    "content_hash": block.content_hash,
                })
            })
            .collect(),
    )
}

fn study_value(study: deepref_postgres::StudyDetailRecord) -> Result<Value, AssistantFailure> {
    let reports = study
        .reports
        .into_iter()
        .map(|report| {
            json!({
                "report_id": report.report_id,
                "title": report.title,
                "abstract_text": report.abstract_text.as_deref().map(|value| bounded_text(value, MAX_REPORT_ABSTRACT_CHARS)),
                "publication_year": report.publication_year,
                "role": report.role.as_str(),
                "assigned_at": report.assigned_at,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "id": study.study.id,
        "project_id": study.study.project_id,
        "title": study.study.title,
        "design": study.study.design.map(StudyDesign::as_str),
        "design_context": study.study.design_context,
        "revision": study.study.revision,
        "created_at": study.study.created_at,
        "updated_at": study.study.updated_at,
        "reports": reports,
        "tool_suggestions": study.tool_suggestions,
    }))
}

fn bounded_text(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn map_parse_error(error: AgentToolParseError) -> ApiError {
    match error {
        AgentToolParseError::MalformedRequest | AgentToolParseError::UnknownTool => {
            ApiError::BadRequest("tool request is malformed or not in the allowlist".to_owned())
        }
    }
}

fn map_runtime_error(error: AgentToolError) -> ApiError {
    match error {
        AgentToolError::Forbidden => ApiError::Forbidden("assistant tool is forbidden".to_owned()),
        AgentToolError::InvalidProjectScope
        | AgentToolError::InvalidArguments
        | AgentToolError::InvalidActor
        | AgentToolError::MalformedRequest
        | AgentToolError::UnknownTool => ApiError::BadRequest("tool request is invalid".to_owned()),
        AgentToolError::InvalidOutput | AgentToolError::ExecutionFailed => {
            ApiError::Internal(anyhow::anyhow!("assistant tool execution failed"))
        }
    }
}

fn read_failure(error: deepref_postgres::AgentReadError) -> AssistantFailure {
    match error {
        deepref_postgres::AgentReadError::NotFound => AssistantFailure::NotFound,
        deepref_postgres::AgentReadError::Database(_)
        | deepref_postgres::AgentReadError::InvalidData => AssistantFailure::Internal,
    }
}

fn protocol_failure(error: deepref_postgres::ProtocolError) -> AssistantFailure {
    match error {
        deepref_postgres::ProtocolError::ProjectNotFound
        | deepref_postgres::ProtocolError::NotFound => AssistantFailure::NotFound,
        _ => AssistantFailure::Internal,
    }
}

fn study_failure(error: deepref_postgres::StudyError) -> AssistantFailure {
    match error {
        deepref_postgres::StudyError::ProjectNotFound
        | deepref_postgres::StudyError::StudyNotFound
        | deepref_postgres::StudyError::ReportNotInProject => AssistantFailure::NotFound,
        _ => AssistantFailure::Internal,
    }
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
