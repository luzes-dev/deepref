//! Assistant sync tool-command path: the closed tool catalog, deterministic
//! tool execution, conversations, and the synchronous chat stream for tool
//! commands. Free-form turns run durably (see [`super::runs`]); run
//! observation lives in [`super::run_events`].

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{KeepAlive, Sse},
    },
};
use chrono::{DateTime, Utc};
use deepref_ai::{
    AgentDispatch, AgentProposalOperation, AgentProposalReceipt, AgentReadOperation, AgentRuntime,
    AgentTool, AgentToolError, AgentToolExecutor, AgentToolName, AssistantChatMessage,
    AssistantDispatcher, AssistantRole, AssistantStreamEvent, AssistantToolCall,
    AssistantToolOutput, AssistantToolResult, AssistantTurnInput, BoundedAgentJson,
    run_assistant_react_turn,
};
use deepref_domain::{
    Actor, DocumentBlockId, DocumentId, ProjectId, RecordId, ReportId, ScreeningStage, StudyDesign,
    StudyId,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use utoipa::ToSchema;
use uuid::Uuid;

use super::runs::AssistantRunDto;
use super::{
    AssistantFailure, assistant_error_frame, budget_exceeded_error, execute_proposal,
    map_conversation_error, provider_failed_error, stream_event_frame, subscription_limit_error,
    validate_project_id,
};
use crate::{
    error::{ApiError, ErrorResponse},
    routes::actor::extract_actor,
    state::AppState,
};
const MAX_BLOCK_TEXT_CHARS: usize = 2_000;
const MAX_REPORT_ABSTRACT_CHARS: usize = 4_000;
/// Longest chat message accepted from the user, in characters.
pub(crate) const MAX_ASSISTANT_MESSAGE_CHARS: usize = 4_000;

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
pub(crate) enum AssistantScreeningStageDto {
    TitleAbstract,
    FullText,
}

impl From<AssistantScreeningStageDto> for ScreeningStage {
    fn from(value: AssistantScreeningStageDto) -> Self {
        match value {
            AssistantScreeningStageDto::TitleAbstract => Self::TitleAbstract,
            AssistantScreeningStageDto::FullText => Self::FullText,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectArgsDto {
    project_id: Uuid,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReportArgsDto {
    project_id: Uuid,
    report_id: Uuid,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentBlocksArgsDto {
    project_id: Uuid,
    document_id: Uuid,
    block_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchDocumentArgsDto {
    project_id: Uuid,
    document_id: Uuid,
    query: String,
    limit: u16,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchProjectReportsArgsDto {
    project_id: Uuid,
    query: String,
    limit: u16,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StudyArgsDto {
    project_id: Uuid,
    study_id: Uuid,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListStudiesArgsDto {
    project_id: Uuid,
    /// 1 to 50; defaults to 25.
    #[serde(default)]
    limit: Option<u16>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AppraisalArgsDto {
    project_id: Uuid,
    report_id: Uuid,
    definition_id: String,
    definition_version: u32,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScreeningDecisionArgsDto {
    project_id: Uuid,
    report_id: Uuid,
    stage: AssistantScreeningStageDto,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DuplicateMergeArgsDto {
    project_id: Uuid,
    source_record_id: Uuid,
    candidate_report_id: Uuid,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(
    tag = "tool",
    content = "args",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum AssistantToolRequest {
    GetProjectProtocol(ProjectArgsDto),
    GetReport(ReportArgsDto),
    ReadDocumentBlocks(DocumentBlocksArgsDto),
    SearchDocument(SearchDocumentArgsDto),
    SearchProjectReports(SearchProjectReportsArgsDto),
    GetScreeningState(ReportArgsDto),
    GetStudy(StudyArgsDto),
    ListStudies(ListStudiesArgsDto),
    GetAppraisal(AppraisalArgsDto),
    ProposeScreeningDecision(ScreeningDecisionArgsDto),
    ProposeDuplicateMerge(DuplicateMergeArgsDto),
    ProposeStudyGrouping(ReportArgsDto),
    ProposeClassification(StudyArgsDto),
    ProposeExtraction(StudyArgsDto),
    ProposeAppraisalAnswer(AppraisalArgsDto),
}

impl AssistantToolRequest {
    fn into_agent_tool(self) -> AgentTool {
        match self {
            Self::GetProjectProtocol(args) => {
                AgentTool::GetProjectProtocol(deepref_ai::ProjectToolArgs {
                    project_id: ProjectId::new(args.project_id),
                })
            }
            Self::GetReport(args) => AgentTool::GetReport(report_args(args)),
            Self::ReadDocumentBlocks(args) => {
                AgentTool::ReadDocumentBlocks(deepref_ai::DocumentBlocksToolArgs {
                    project_id: ProjectId::new(args.project_id),
                    document_id: DocumentId::new(args.document_id),
                    block_ids: args
                        .block_ids
                        .into_iter()
                        .map(DocumentBlockId::new)
                        .collect(),
                })
            }
            Self::SearchDocument(args) => {
                AgentTool::SearchDocument(deepref_ai::SearchDocumentToolArgs {
                    project_id: ProjectId::new(args.project_id),
                    document_id: DocumentId::new(args.document_id),
                    query: args.query,
                    limit: args.limit,
                })
            }
            Self::SearchProjectReports(args) => {
                AgentTool::SearchProjectReports(deepref_ai::SearchProjectReportsToolArgs {
                    project_id: ProjectId::new(args.project_id),
                    query: args.query,
                    limit: args.limit,
                })
            }
            Self::GetScreeningState(args) => {
                AgentTool::GetScreeningState(deepref_ai::ScreeningStateToolArgs {
                    project_id: ProjectId::new(args.project_id),
                    report_id: ReportId::new(args.report_id),
                })
            }
            Self::GetStudy(args) => AgentTool::GetStudy(study_args(args)),
            Self::ListStudies(args) => AgentTool::ListStudies(deepref_ai::ListStudiesToolArgs {
                project_id: ProjectId::new(args.project_id),
                limit: args.limit,
            }),
            Self::GetAppraisal(args) => AgentTool::GetAppraisal(appraisal_args(args)),
            Self::ProposeScreeningDecision(args) => {
                AgentTool::ProposeScreeningDecision(deepref_ai::ScreeningDecisionProposalArgs {
                    project_id: ProjectId::new(args.project_id),
                    report_id: ReportId::new(args.report_id),
                    stage: args.stage.into(),
                })
            }
            Self::ProposeDuplicateMerge(args) => {
                AgentTool::ProposeDuplicateMerge(deepref_ai::DuplicateMergeProposalArgs {
                    project_id: ProjectId::new(args.project_id),
                    source_record_id: RecordId::new(args.source_record_id),
                    candidate_report_id: ReportId::new(args.candidate_report_id),
                })
            }
            Self::ProposeStudyGrouping(args) => {
                AgentTool::ProposeStudyGrouping(deepref_ai::StudyGroupingProposalArgs {
                    project_id: ProjectId::new(args.project_id),
                    report_id: ReportId::new(args.report_id),
                })
            }
            Self::ProposeClassification(args) => {
                AgentTool::ProposeClassification(deepref_ai::ClassificationProposalArgs {
                    project_id: ProjectId::new(args.project_id),
                    study_id: StudyId::new(args.study_id),
                })
            }
            Self::ProposeExtraction(args) => {
                AgentTool::ProposeExtraction(deepref_ai::ExtractionProposalArgs {
                    project_id: ProjectId::new(args.project_id),
                    study_id: StudyId::new(args.study_id),
                })
            }
            Self::ProposeAppraisalAnswer(args) => {
                AgentTool::ProposeAppraisalAnswer(deepref_ai::AppraisalAnswerProposalArgs {
                    project_id: ProjectId::new(args.project_id),
                    report_id: ReportId::new(args.report_id),
                    definition_id: args.definition_id,
                    definition_version: args.definition_version,
                })
            }
        }
    }
}

fn report_args(args: ReportArgsDto) -> deepref_ai::ReportToolArgs {
    deepref_ai::ReportToolArgs {
        project_id: ProjectId::new(args.project_id),
        report_id: ReportId::new(args.report_id),
    }
}

fn study_args(args: StudyArgsDto) -> deepref_ai::StudyToolArgs {
    deepref_ai::StudyToolArgs {
        project_id: ProjectId::new(args.project_id),
        study_id: StudyId::new(args.study_id),
    }
}

fn appraisal_args(args: AppraisalArgsDto) -> deepref_ai::AppraisalToolArgs {
    deepref_ai::AppraisalToolArgs {
        project_id: ProjectId::new(args.project_id),
        report_id: ReportId::new(args.report_id),
        definition_id: args.definition_id,
        definition_version: args.definition_version,
    }
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
    let tool = serde_json::from_value::<AssistantToolRequest>(body)
        .map_err(|_| {
            ApiError::BadRequest("tool request is malformed or not in the allowlist".to_owned())
        })?
        .into_agent_tool();
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
            Err(error) => return Err(map_assistant_failure(error)),
        },
        AgentDispatch::Proposal(future) => match future.await {
            Ok(AgentProposalReceipt { review_run_id }) => AssistantToolResponse::ReviewRun {
                review_run_id,
                status_path: format!("/projects/{project_id}/review-runs/{review_run_id}"),
            },
            Err(error) => return Err(map_assistant_failure(error)),
        },
    };
    Ok(Json(response))
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
        AgentToolName::ListStudies => {
            "List the project's studies with their design and report counts."
        }
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

struct ProjectAgentToolExecutor {
    state: AppState,
    actor: Actor,
}

impl ProjectAgentToolExecutor {
    fn new(state: AppState, actor: Actor) -> Self {
        Self { state, actor }
    }
}

impl AgentToolExecutor for ProjectAgentToolExecutor {
    type Error = AssistantFailure;

    fn execute_read<'a>(
        &'a self,
        operation: AgentReadOperation,
    ) -> deepref_ai::AgentReadFuture<'a, Self::Error> {
        let state = self.state.clone();
        Box::pin(async move {
            let value = execute_read(&state, operation).await?;
            BoundedAgentJson::new(value).map_err(|_| AssistantFailure::Internal)
        })
    }

    fn create_proposal<'a>(
        &'a self,
        operation: AgentProposalOperation,
    ) -> deepref_ai::AgentProposalFuture<'a, Self::Error> {
        let state = self.state.clone();
        let actor = self.actor.clone();
        Box::pin(async move {
            let run = execute_proposal(&state, operation, actor).await?;
            Ok(AgentProposalReceipt {
                review_run_id: run.id.as_uuid(),
            })
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
        AgentReadOperation::ListStudies(args) => deepref_postgres::list_agent_studies(
            &state.pool,
            args.project_id.as_uuid(),
            i64::from(args.limit.unwrap_or(25)),
        )
        .await
        .map_err(read_failure),
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
    let mut value = json!({
        "id": report.id,
        "project_id": report.project_id,
        "title": report.title,
        "abstract_text": report.abstract_text.as_deref().map(|value| bounded_text(value, abstract_limit)),
        "publication_year": report.publication_year,
        "journal": report.journal,
        "url": report.url,
        "identifiers": report.identifiers,
    });
    if let Some(documents) = report.documents {
        // Full text exists only when a document has been parsed (status available).
        let full_text_available = documents
            .iter()
            .any(|document| document.status == "available");
        value["full_text_available"] = json!(full_text_available);
        value["documents"] = json!(documents);
    }
    value
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
                "title": report.title.as_deref().map(deepref_postgres::decode_html_entities),
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
        "title": deepref_postgres::decode_html_entities(&study.study.title),
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

fn map_assistant_failure(error: AssistantFailure) -> ApiError {
    match error {
        AssistantFailure::NotFound => ApiError::NotFound("scoped resource not found".to_owned()),
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

fn map_runtime_error(error: AgentToolError) -> ApiError {
    match error {
        AgentToolError::Forbidden => ApiError::Forbidden("assistant tool is forbidden".to_owned()),
        AgentToolError::InvalidProjectScope
        | AgentToolError::InvalidArguments
        | AgentToolError::InvalidActor
        | AgentToolError::MalformedRequest
        | AgentToolError::UnknownTool => ApiError::BadRequest("tool request is invalid".to_owned()),
        AgentToolError::NotConfigured => ApiError::Configuration(
            "The assistant needs an AI provider, which is not configured for this workspace."
                .to_owned(),
        ),
        AgentToolError::BudgetExceeded => budget_exceeded_error(),
        AgentToolError::SubscriptionLimit => subscription_limit_error(),
        AgentToolError::ProviderFailed => provider_failed_error(),
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

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AssistantConversationDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AssistantMessageDto {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub role: String,
    pub content: String,
    #[schema(value_type = Object)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Value>,
    #[schema(value_type = Object)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_results: Option<Value>,
    #[schema(value_type = Object)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateAssistantConversationRequestDto {
    title: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AssistantChatRequestDto {
    conversation_id: Uuid,
    message: String,
}

fn conversation_dto(
    record: &deepref_postgres::AssistantConversationRecord,
) -> AssistantConversationDto {
    AssistantConversationDto {
        id: record.id,
        project_id: record.project_id,
        title: record.title.clone(),
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

#[allow(clippy::too_many_lines)]
fn message_dto(record: &deepref_postgres::AssistantMessageRecord) -> AssistantMessageDto {
    AssistantMessageDto {
        id: record.id,
        conversation_id: record.conversation_id,
        role: record.role.clone(),
        content: record.content.clone(),
        tool_calls: record.tool_calls.clone(),
        tool_results: record.tool_results.clone(),
        metadata: record.metadata.clone(),
        created_at: record.created_at,
    }
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/assistant/conversations",
    operation_id = "listProjectAssistantConversations",
    tag = "assistant",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    responses(
        (status = 200, description = "Assistant conversations ordered by most recent activity", body = Vec<AssistantConversationDto>),
        (status = 400, description = "Invalid project identifier", body = ErrorResponse),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_conversations(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<Vec<AssistantConversationDto>>, ApiError> {
    validate_project_id(project_id)?;
    if !deepref_postgres::project_exists(&state.pool, project_id).await? {
        return Err(ApiError::NotFound("project not found".to_owned()));
    }
    let conversations = deepref_postgres::list_assistant_conversations(&state.pool, project_id)
        .await
        .map_err(map_conversation_error)?;
    Ok(Json(conversations.iter().map(conversation_dto).collect()))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/assistant/conversations",
    operation_id = "createProjectAssistantConversation",
    tag = "assistant",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    request_body = CreateAssistantConversationRequestDto,
    responses(
        (status = 201, description = "Conversation created", body = AssistantConversationDto),
        (status = 400, description = "Invalid conversation title", body = ErrorResponse),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn create_conversation(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Json(body): Json<CreateAssistantConversationRequestDto>,
) -> Result<(StatusCode, Json<AssistantConversationDto>), ApiError> {
    validate_project_id(project_id)?;
    if !deepref_postgres::project_exists(&state.pool, project_id).await? {
        return Err(ApiError::NotFound("project not found".to_owned()));
    }
    let conversation =
        deepref_postgres::create_assistant_conversation(&state.pool, project_id, &body.title)
            .await
            .map_err(map_conversation_error)?;
    Ok((StatusCode::CREATED, Json(conversation_dto(&conversation))))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/assistant/conversations/{conversation_id}/messages",
    operation_id = "listProjectAssistantConversationMessages",
    tag = "assistant",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("conversation_id" = Uuid, Path, description = "Conversation identifier")
    ),
    responses(
        (status = 200, description = "Conversation messages in chronological order", body = Vec<AssistantMessageDto>),
        (status = 400, description = "Invalid identifiers", body = ErrorResponse),
        (status = 404, description = "Project or conversation not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_conversation_messages(
    State(state): State<AppState>,
    Path((project_id, conversation_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<AssistantMessageDto>>, ApiError> {
    validate_project_id(project_id)?;
    if !deepref_postgres::project_exists(&state.pool, project_id).await? {
        return Err(ApiError::NotFound("project not found".to_owned()));
    }
    deepref_postgres::get_assistant_conversation(&state.pool, project_id, conversation_id)
        .await
        .map_err(map_conversation_error)?;
    let messages = deepref_postgres::list_assistant_messages(&state.pool, conversation_id)
        .await
        .map_err(map_conversation_error)?;
    Ok(Json(messages.iter().map(message_dto).collect()))
}

#[utoipa::path(
    delete,
    path = "/projects/{project_id}/assistant/conversations/{conversation_id}",
    operation_id = "deleteProjectAssistantConversation",
    tag = "assistant",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("conversation_id" = Uuid, Path, description = "Conversation identifier")
    ),
    responses(
        (status = 204, description = "Conversation deleted"),
        (status = 400, description = "Invalid identifiers", body = ErrorResponse),
        (status = 404, description = "Project or conversation not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn delete_conversation(
    State(state): State<AppState>,
    Path((project_id, conversation_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    validate_project_id(project_id)?;
    if !deepref_postgres::project_exists(&state.pool, project_id).await? {
        return Err(ApiError::NotFound("project not found".to_owned()));
    }
    let deleted =
        deepref_postgres::delete_assistant_conversation(&state.pool, project_id, conversation_id)
            .await
            .map_err(map_conversation_error)?;
    if !deleted {
        return Err(ApiError::NotFound(
            "assistant conversation not found".to_owned(),
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

struct ConversationAgentDispatcher {
    project_id: ProjectId,
    state: AppState,
    actor: Actor,
}

impl AssistantDispatcher for ConversationAgentDispatcher {
    fn execute_tool<'a>(
        &'a self,
        tool_name: &'a str,
        args: Value,
    ) -> Pin<Box<dyn Future<Output = Result<AssistantToolOutput, AgentToolError>> + Send + 'a>>
    {
        let project_id = self.project_id;
        let state = self.state.clone();
        let actor = self.actor.clone();
        Box::pin(async move {
            dispatch_assistant_tool(project_id, &state, actor, tool_name, args).await
        })
    }
}

async fn dispatch_assistant_tool(
    project_id: ProjectId,
    state: &AppState,
    actor: Actor,
    tool_name: &str,
    args: Value,
) -> Result<AssistantToolOutput, AgentToolError> {
    if tool_name == "trigger_workflow" {
        return trigger_workflow(state, project_id, actor, &args).await;
    }
    let parsed_name = AgentToolName::parse(tool_name).ok_or(AgentToolError::UnknownTool)?;
    let tool = AgentTool::from_name_and_args(parsed_name, args)
        .map_err(|_| AgentToolError::MalformedRequest)?;
    tool.validate()?;
    if parsed_name.is_read() {
        let value = execute_read(state, tool.into_read_operation()?)
            .await
            .map_err(assistant_failure_to_tool_error)?;
        let bounded = BoundedAgentJson::new(value).map_err(|_| AgentToolError::InvalidOutput)?;
        return Ok(AssistantToolOutput::Read(bounded.into_value()));
    }
    let run = execute_proposal(state, tool.into_proposal_operation()?, actor)
        .await
        .map_err(assistant_failure_to_tool_error)?;
    Ok(AssistantToolOutput::Proposal {
        review_run_id: run.id.as_uuid(),
        status_path: format!(
            "/projects/{}/review-runs/{}",
            project_id.as_uuid(),
            run.id.as_uuid()
        ),
    })
}

async fn trigger_workflow(
    state: &AppState,
    project_id: ProjectId,
    actor: Actor,
    args: &Value,
) -> Result<AssistantToolOutput, AgentToolError> {
    let definition_id = args
        .get("definition_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or(AgentToolError::InvalidArguments)?;
    let request = deepref_application::automations::StartAutomationManually::new(
        project_id,
        definition_id,
        Uuid::new_v4().to_string(),
        actor,
    )
    .map_err(|_| AgentToolError::InvalidArguments)?;
    let result = deepref_postgres::start_automation_manually(&state.pool, &request)
        .await
        .map_err(|_| AgentToolError::ExecutionFailed)?;
    Ok(AssistantToolOutput::WorkflowTriggered {
        run_id: result.run_id.as_uuid(),
        job_id: result.job_id,
        created: result.created,
    })
}

fn assistant_failure_to_tool_error(error: AssistantFailure) -> AgentToolError {
    match error {
        AssistantFailure::NotFound => AgentToolError::InvalidArguments,
        AssistantFailure::Conflict | AssistantFailure::Internal => AgentToolError::ExecutionFailed,
    }
}

fn is_tool_command(message: &str) -> bool {
    serde_json::from_str::<Value>(message)
        .ok()
        .is_some_and(|value| value.get("tool").and_then(Value::as_str).is_some())
}

fn map_turn_error(error: AgentToolError) -> ApiError {
    match error {
        AgentToolError::Forbidden => ApiError::Forbidden("assistant tool is forbidden".to_owned()),
        AgentToolError::InvalidProjectScope
        | AgentToolError::InvalidArguments
        | AgentToolError::InvalidActor
        | AgentToolError::MalformedRequest
        | AgentToolError::UnknownTool => {
            ApiError::BadRequest("assistant turn request is invalid".to_owned())
        }
        AgentToolError::NotConfigured => ApiError::Configuration(
            "The assistant needs an AI provider, which is not configured for this workspace."
                .to_owned(),
        ),
        AgentToolError::BudgetExceeded => budget_exceeded_error(),
        AgentToolError::SubscriptionLimit => subscription_limit_error(),
        AgentToolError::ProviderFailed => provider_failed_error(),
        AgentToolError::InvalidOutput | AgentToolError::ExecutionFailed => {
            ApiError::Internal(anyhow::anyhow!("assistant turn failed"))
        }
    }
}

fn history_chat_message(
    record: &deepref_postgres::AssistantMessageRecord,
) -> Option<AssistantChatMessage> {
    let role = AssistantRole::parse(&record.role)?;
    let tool_calls = record
        .tool_calls
        .as_ref()
        .and_then(|value| serde_json::from_value::<Vec<AssistantToolCall>>(value.clone()).ok());
    let tool_results = record
        .tool_results
        .as_ref()
        .and_then(|value| serde_json::from_value::<Vec<AssistantToolResult>>(value.clone()).ok());
    Some(AssistantChatMessage {
        role,
        content: record.content.clone(),
        tool_calls,
        tool_results,
        metadata: record.metadata.clone(),
    })
}

async fn persist_assistant_turn(
    pool: &sqlx::PgPool,
    conversation_id: Uuid,
    message: &AssistantChatMessage,
) -> Result<(), deepref_postgres::AssistantError> {
    let record = deepref_postgres::AppendAssistantMessage {
        id: None,
        conversation_id,
        role: message.role.as_str().to_owned(),
        content: message.content.clone(),
        tool_calls: message
            .tool_calls
            .as_ref()
            .map(|value| serde_json::to_value(value).unwrap_or(Value::Null)),
        tool_results: message
            .tool_results
            .as_ref()
            .map(|value| serde_json::to_value(value).unwrap_or(Value::Null)),
        metadata: message.metadata.clone(),
    };
    deepref_postgres::append_assistant_message(pool, &record)
        .await
        .map(|_| ())
}

type AssistantSseState = (
    tokio::sync::mpsc::Receiver<AssistantStreamEvent>,
    Option<AssistantStreamEvent>,
    Option<tokio::sync::oneshot::Receiver<Result<(), AgentToolError>>>,
);

#[utoipa::path(
    post,
    path = "/projects/{project_id}/assistant/chat",
    operation_id = "chatWithProjectAssistant",
    tag = "assistant",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    request_body = AssistantChatRequestDto,
    responses(
        (status = 200, description = "Server-sent assistant stream for deterministic tool commands", content_type = "text/event-stream", body = String),
        (status = 202, description = "Free-form turn accepted for durable execution", body = AssistantRunDto),
        (status = 400, description = "Malformed chat request or tool command", body = ErrorResponse),
        (status = 403, description = "Tool is forbidden by the project policy", body = ErrorResponse),
        (status = 404, description = "Project or conversation not found", body = ErrorResponse),
        (status = 413, description = "Message is longer than 4000 characters", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse),
        (status = 503, description = "No AI provider is configured for free-form replies", body = ErrorResponse)
    )
)]
#[allow(clippy::too_many_lines)]
pub(crate) async fn chat(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<AssistantChatRequestDto>,
) -> Result<Response, ApiError> {
    validate_project_id(project_id)?;
    let actor = extract_actor(&headers)?;
    if !deepref_postgres::project_exists(&state.pool, project_id).await? {
        return Err(ApiError::NotFound("project not found".to_owned()));
    }
    let user_message = body.message.trim().to_owned();
    if user_message.is_empty() {
        return Err(ApiError::BadRequest("message must not be blank".to_owned()));
    }
    if user_message.chars().count() > MAX_ASSISTANT_MESSAGE_CHARS {
        return Err(ApiError::PayloadTooLarge(format!(
            "This message is too long. Messages can be up to {MAX_ASSISTANT_MESSAGE_CHARS} characters; shorten it or split it into two messages."
        )));
    }
    deepref_postgres::get_assistant_conversation(&state.pool, project_id, body.conversation_id)
        .await
        .map_err(map_conversation_error)?;
    let free_form = !is_tool_command(&user_message);
    if free_form {
        return super::runs::chat_durable(
            state,
            project_id,
            actor,
            body.conversation_id,
            user_message,
        )
        .await
        .map(|dto| (StatusCode::ACCEPTED, Json(dto)).into_response());
    }
    let history_records =
        deepref_postgres::list_assistant_messages(&state.pool, body.conversation_id)
            .await
            .map_err(map_conversation_error)?;
    let history = history_records
        .iter()
        .filter_map(history_chat_message)
        .collect::<Vec<_>>();

    deepref_postgres::append_assistant_message(
        &state.pool,
        &deepref_postgres::AppendAssistantMessage {
            id: None,
            conversation_id: body.conversation_id,
            role: "user".to_owned(),
            content: user_message.clone(),
            tool_calls: None,
            tool_results: None,
            metadata: None,
        },
    )
    .await
    .map_err(map_conversation_error)?;

    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<AssistantStreamEvent>(64);
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    let turn_project_id = ProjectId::new(project_id);
    let turn_state = state.clone();
    // Only deterministic tool commands reach this synchronous path now;
    // free-form turns run durably in the worker (see chat_durable).
    tokio::spawn(async move {
        let dispatcher = ConversationAgentDispatcher {
            project_id: turn_project_id,
            state: turn_state.clone(),
            actor: actor.clone(),
        };
        let input = AssistantTurnInput {
            project_id: turn_project_id,
            actor,
            project_policy: deepref_ai::ProjectAiPolicy::default(),
            history: &history,
            user_message: &user_message,
        };
        let outcome = run_assistant_react_turn(input, &dispatcher, &event_tx).await;
        if let Ok(message) = &outcome
            && let Err(error) =
                persist_assistant_turn(&turn_state.pool, body.conversation_id, message).await
        {
            tracing::warn!(error = %error, "failed to persist assistant turn");
        }
        drop(event_tx);
        let _ = result_tx.send(outcome.map(|_| ()));
    });

    match event_rx.recv().await {
        Some(first_event) => {
            let stream = futures::stream::unfold(
                (event_rx, Some(first_event), Some(result_rx)),
                |(mut rx, pending, result): AssistantSseState| async move {
                    if let Some(event) = pending {
                        return Some((stream_event_frame(&event), (rx, None, result)));
                    }
                    match rx.recv().await {
                        Some(event) => Some((stream_event_frame(&event), (rx, None, result))),
                        None => match result {
                            Some(receiver) => match receiver.await {
                                Ok(Ok(())) | Err(_) => None,
                                Ok(Err(error)) => {
                                    Some((assistant_error_frame(error), (rx, None, None)))
                                }
                            },
                            None => None,
                        },
                    }
                },
            );
            Ok(Sse::new(stream)
                .keep_alive(KeepAlive::default())
                .into_response())
        }
        None => {
            let outcome = result_rx
                .await
                .unwrap_or(Err(AgentToolError::ExecutionFailed));
            Err(map_turn_error(
                outcome.err().unwrap_or(AgentToolError::ExecutionFailed),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_records_map_into_chat_messages() {
        let record = deepref_postgres::AssistantMessageRecord {
            id: Uuid::nil(),
            conversation_id: Uuid::nil(),
            role: "assistant".to_owned(),
            content: "done".to_owned(),
            tool_calls: Some(json!([{ "id": "call-1", "tool": "get_report", "args": {} }])),
            tool_results: Some(json!([{
                "tool_call_id": "call-1",
                "tool": "get_report",
                "output": {}
            }])),
            metadata: Some(json!({ "message_id": Uuid::nil() })),
            created_at: Utc::now(),
        };

        let mapped = history_chat_message(&record).expect("assistant record must map");
        assert_eq!(mapped.role, AssistantRole::Assistant);
        assert_eq!(mapped.tool_calls.expect("tool calls").len(), 1);
        assert_eq!(mapped.tool_results.expect("tool results").len(), 1);
    }

    #[test]
    fn unknown_roles_and_malformed_tool_payloads_are_dropped_from_history() {
        let mut record = deepref_postgres::AssistantMessageRecord {
            id: Uuid::nil(),
            conversation_id: Uuid::nil(),
            role: "moderator".to_owned(),
            content: "nope".to_owned(),
            tool_calls: None,
            tool_results: None,
            metadata: None,
            created_at: Utc::now(),
        };
        assert!(history_chat_message(&record).is_none());

        record.role = "assistant".to_owned();
        record.tool_calls = Some(json!([{ "unexpected": true }]));
        let mapped = history_chat_message(&record).expect("assistant record must map");
        assert!(mapped.tool_calls.is_none());
    }
}
