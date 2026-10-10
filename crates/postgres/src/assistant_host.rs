//! Project-scoped read execution for assistant tools, usable without HTTP.
//!
//! [`PostgresAssistantToolHost`] implements DeepRef's
//! [`AssistantToolHost`][deepref_ai::runtime::AssistantToolHost] directly
//! against the database pool, so both the HTTP transport and the durable
//! worker execute reads through one implementation. The adapter injects the
//! host project scope into arguments before validation; this host additionally
//! refuses any catalog operation whose declared project differs from it.
//!
//! Reads run immediately without policy checks, mirroring the interactive
//! loop: only write tools go through the `PolicyEngine`. Proposal tools
//! never reach this host; they become `PlanAction` data in the turn.

use deepref_ai::{
    AgentReadOperation, AgentToolName, AiError, AiFuture, AppraisalToolArgs, BoundedAgentJson,
    DocumentBlocksToolArgs, ListStudiesToolArgs, ProjectToolArgs, ReportToolArgs,
    ScreeningStateToolArgs, SearchDocumentToolArgs, SearchProjectReportsToolArgs, StudyToolArgs,
    TOOL_LIST_REPORTS, TOOL_PROJECT_OVERVIEW, runtime::AssistantToolHost, truncate_chars,
};
use deepref_domain::{ProjectId, StudyDesign};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

/// Model-facing read bound helpers shared with the HTTP transport.
const MAX_BLOCK_TEXT_CHARS: usize = 2_000;
const MAX_REPORT_ABSTRACT_CHARS: usize = 4_000;

#[derive(Debug, Clone)]
pub struct PostgresAssistantToolHost {
    pool: PgPool,
}

impl PostgresAssistantToolHost {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }

    async fn read(&self, project_id: ProjectId, tool: &str, args: Value) -> Result<Value, AiError> {
        match tool {
            TOOL_PROJECT_OVERVIEW => {
                let project = args
                    .get("project_id")
                    .and_then(Value::as_str)
                    .and_then(|text| Uuid::parse_str(text).ok())
                    .ok_or_else(|| AiError::InvalidContext("missing project".to_owned()))?;
                require_scope(project_id, project)?;
                crate::get_agent_project_overview(&self.pool, project)
                    .await
                    .map_err(read_error)
            }
            TOOL_LIST_REPORTS => {
                let project = args
                    .get("project_id")
                    .and_then(Value::as_str)
                    .and_then(|text| Uuid::parse_str(text).ok())
                    .ok_or_else(|| AiError::InvalidContext("missing project".to_owned()))?;
                require_scope(project_id, project)?;
                let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(25);
                crate::list_agent_reports_by_screening(
                    &self.pool,
                    project,
                    args.get("status").and_then(Value::as_str),
                    limit,
                )
                .await
                .map_err(read_error)
            }
            other => {
                let name = AgentToolName::parse(other)
                    .ok_or_else(|| AiError::InvalidContext("unknown tool".to_owned()))?;
                let tool = deepref_ai::AgentTool::from_name_and_args(name, args).map_err(|_| {
                    AiError::InvalidContext("the arguments are malformed".to_owned())
                })?;
                tool.validate()
                    .map_err(|_| AiError::InvalidContext("the arguments are invalid".to_owned()))?;
                require_scope(project_id, tool.project_id().as_uuid())?;
                let operation = tool
                    .into_read_operation()
                    .map_err(|_| AiError::InvalidContext("not a read tool".to_owned()))?;
                let value = execute_read_operation(&self.pool, operation).await?;
                BoundedAgentJson::new(value)
                    .map(BoundedAgentJson::into_value)
                    .map_err(|_| AiError::InvalidContext("the result was too large".to_owned()))
            }
        }
    }
}

impl AssistantToolHost for PostgresAssistantToolHost {
    fn execute_read<'a>(
        &'a self,
        project_id: ProjectId,
        tool: &'a str,
        args: Value,
    ) -> AiFuture<'a, Value> {
        Box::pin(async move { self.read(project_id, tool, args).await })
    }
}

fn require_scope(host: ProjectId, declared: Uuid) -> Result<(), AiError> {
    if host.as_uuid() != declared {
        return Err(AiError::InvalidContext("invalid project scope".to_owned()));
    }
    Ok(())
}

/// The single [`AgentReadError`] → [`AiError`] mapping for this host. Every
/// read below reports `AgentReadError` (native protocol/study errors are
/// folded into it first) and converts once at the boundary.
///
/// This free function — not `impl From<AgentReadError> for AiError` — is the
/// orphan-rule-compliant spelling: both the `From` trait and `AiError` are
/// foreign to this crate, so that impl cannot live here.
fn read_error(error: crate::AgentReadError) -> AiError {
    match error {
        crate::AgentReadError::NotFound => {
            AiError::InvalidContext("that item was not found in this project".to_owned())
        }
        crate::AgentReadError::InvalidData => {
            AiError::InvalidContext("invalid argument".to_owned())
        }
        crate::AgentReadError::Database(_) => AiError::Persistence("read failed".to_owned()),
    }
}

/// Folds native protocol errors into the shared read error. Not-found stays
/// not-found; database failures stay database failures (previously they were
/// downgraded to a model-visible "the tool failed", unlike every other read
/// arm — now they surface as retryable persistence errors like the rest);
/// any other domain failure is model-visible invalid data.
fn protocol_read_error(error: crate::ProtocolError) -> crate::AgentReadError {
    match error {
        crate::ProtocolError::ProjectNotFound | crate::ProtocolError::NotFound => {
            crate::AgentReadError::NotFound
        }
        crate::ProtocolError::Database(error) => crate::AgentReadError::Database(error),
        _ => crate::AgentReadError::InvalidData,
    }
}

/// Folds native study errors into the shared read error, with the same
/// not-found / database / invalid-data split as
/// [`protocol_read_error`].
fn study_read_error(error: crate::StudyError) -> crate::AgentReadError {
    match error {
        crate::StudyError::ProjectNotFound
        | crate::StudyError::StudyNotFound
        | crate::StudyError::ReportNotInProject => crate::AgentReadError::NotFound,
        crate::StudyError::Database(error) => crate::AgentReadError::Database(error),
        _ => crate::AgentReadError::InvalidData,
    }
}

async fn execute_read_operation(
    pool: &PgPool,
    operation: AgentReadOperation,
) -> Result<Value, AiError> {
    dispatch_read(pool, operation).await.map_err(read_error)
}

/// Dispatcher table for catalog reads: one thin arm per operation, each a
/// single handler call. Every handler reports [`crate::AgentReadError`] so
/// the single [`read_error`] conversion above serves all arms, including the
/// protocol/study arms whose native errors are folded in first.
async fn dispatch_read(
    pool: &PgPool,
    operation: AgentReadOperation,
) -> Result<Value, crate::AgentReadError> {
    match operation {
        AgentReadOperation::GetProjectProtocol(args) => read_project_protocol(pool, args).await,
        AgentReadOperation::GetReport(args) => read_report(pool, args).await,
        AgentReadOperation::ReadDocumentBlocks(args) => read_document_blocks(pool, args).await,
        AgentReadOperation::SearchDocument(args) => search_document(pool, args).await,
        AgentReadOperation::SearchProjectReports(args) => search_project_reports(pool, args).await,
        AgentReadOperation::GetScreeningState(args) => read_screening_state(pool, args).await,
        AgentReadOperation::GetStudy(args) => read_study(pool, args).await,
        AgentReadOperation::ListStudies(args) => list_studies(pool, args).await,
        AgentReadOperation::GetAppraisal(args) => read_appraisal(pool, args).await,
    }
}

async fn read_project_protocol(
    pool: &PgPool,
    args: ProjectToolArgs,
) -> Result<Value, crate::AgentReadError> {
    let protocol = crate::get_published_protocol(pool, args.project_id.as_uuid())
        .await
        .map_err(protocol_read_error)?;
    protocol_value(protocol)
}

async fn read_report(pool: &PgPool, args: ReportToolArgs) -> Result<Value, crate::AgentReadError> {
    let report =
        crate::get_agent_report(pool, args.project_id.as_uuid(), args.report_id.as_uuid()).await?;
    Ok(report_value(report, MAX_REPORT_ABSTRACT_CHARS))
}

async fn read_document_blocks(
    pool: &PgPool,
    args: DocumentBlocksToolArgs,
) -> Result<Value, crate::AgentReadError> {
    let block_ids = args
        .block_ids
        .iter()
        .map(|block_id| block_id.as_uuid())
        .collect::<Vec<_>>();
    let blocks = crate::read_agent_document_blocks(
        pool,
        args.project_id.as_uuid(),
        args.document_id.as_uuid(),
        &block_ids,
    )
    .await?;
    Ok(blocks_value(blocks))
}

async fn search_document(
    pool: &PgPool,
    args: SearchDocumentToolArgs,
) -> Result<Value, crate::AgentReadError> {
    let blocks = crate::search_agent_document(
        pool,
        args.project_id.as_uuid(),
        args.document_id.as_uuid(),
        &args.query,
        i64::from(args.limit),
    )
    .await?;
    Ok(blocks_value(blocks))
}

async fn search_project_reports(
    pool: &PgPool,
    args: SearchProjectReportsToolArgs,
) -> Result<Value, crate::AgentReadError> {
    let reports = crate::search_agent_reports(
        pool,
        args.project_id.as_uuid(),
        &args.query,
        i64::from(args.limit),
    )
    .await?;
    Ok(Value::Array(
        reports
            .into_iter()
            .map(|report| report_value(report, MAX_REPORT_ABSTRACT_CHARS))
            .collect(),
    ))
}

async fn read_screening_state(
    pool: &PgPool,
    args: ScreeningStateToolArgs,
) -> Result<Value, crate::AgentReadError> {
    let screening =
        crate::get_agent_screening_state(pool, args.project_id.as_uuid(), args.report_id.as_uuid())
            .await?;
    serde_json::to_value(screening).map_err(|_| crate::AgentReadError::InvalidData)
}

async fn read_study(pool: &PgPool, args: StudyToolArgs) -> Result<Value, crate::AgentReadError> {
    let study = crate::get_study(pool, args.project_id.as_uuid(), args.study_id.as_uuid())
        .await
        .map_err(study_read_error)?;
    Ok(study_value(study))
}

async fn list_studies(
    pool: &PgPool,
    args: ListStudiesToolArgs,
) -> Result<Value, crate::AgentReadError> {
    crate::list_agent_studies(
        pool,
        args.project_id.as_uuid(),
        i64::from(args.limit.unwrap_or(25)),
    )
    .await
}

async fn read_appraisal(
    pool: &PgPool,
    args: AppraisalToolArgs,
) -> Result<Value, crate::AgentReadError> {
    let appraisal = crate::get_latest_agent_appraisal(
        pool,
        args.project_id.as_uuid(),
        args.report_id.as_uuid(),
        &args.definition_id,
        i32::try_from(args.definition_version).map_err(|_| crate::AgentReadError::InvalidData)?,
    )
    .await?;
    serde_json::to_value(appraisal).map_err(|_| crate::AgentReadError::InvalidData)
}

fn protocol_value(protocol: crate::ProtocolDocument) -> Result<Value, crate::AgentReadError> {
    let framework =
        serde_json::to_value(protocol.framework).map_err(|_| crate::AgentReadError::InvalidData)?;
    let criteria =
        serde_json::to_value(protocol.criteria).map_err(|_| crate::AgentReadError::InvalidData)?;
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

fn report_value(report: crate::AgentReportRecord, abstract_limit: usize) -> Value {
    let mut value = json!({
        "id": report.id,
        "project_id": report.project_id,
        "title": report.title,
        "abstract_text": report.abstract_text.as_deref().map(|value| truncate_chars(value, abstract_limit)),
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

fn blocks_value(blocks: Vec<crate::AgentDocumentBlockRecord>) -> Value {
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
                    "text": truncate_chars(&block.text, MAX_BLOCK_TEXT_CHARS),
                    "content_hash": block.content_hash,
                })
            })
            .collect(),
    )
}

fn study_value(study: crate::StudyDetailRecord) -> Value {
    let reports = study
        .reports
        .into_iter()
        .map(|report| {
            json!({
                "report_id": report.report_id,
                "title": report.title.as_deref().map(crate::decode_html_entities),
                "abstract_text": report.abstract_text.as_deref().map(|value| truncate_chars(value, MAX_REPORT_ABSTRACT_CHARS)),
                "publication_year": report.publication_year,
                "role": report.role.as_str(),
                "assigned_at": report.assigned_at,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "id": study.study.id,
        "project_id": study.study.project_id,
        "title": crate::decode_html_entities(&study.study.title),
        "design": study.study.design.map(StudyDesign::as_str),
        "design_context": study.study.design_context,
        "revision": study.study.revision,
        "created_at": study.study.created_at,
        "updated_at": study.study.updated_at,
        "reports": reports,
        "tool_suggestions": study.tool_suggestions,
    })
}
