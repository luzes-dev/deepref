//! A small, closed-world runtime for project-scoped agent tools.
//!
//! The model-facing request is an exhaustive enum.  It is converted into one
//! of two narrower operation enums only after the shared [`PolicyEngine`]
//! authorizes it.  In particular, proposal operations cannot become domain
//! writes by choosing a different action at runtime.

use std::{fmt, future::Future, pin::Pin};

use deepref_domain::{
    Actor, DocumentBlockId, DocumentId, ProjectId, RecordId, ReportId, ScreeningStage, StudyId,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    AuthorityTier, PolicyDecision, PolicyEngine, PolicyInput, ProjectAiPolicy, RequestedAction,
};

const MAX_QUERY_LENGTH: usize = 4_096;
const MAX_DEFINITION_ID_LENGTH: usize = 100;
const MAX_OUTPUT_BYTES: usize = 512 * 1024;
const MAX_DOCUMENT_BLOCKS: usize = 200;

/// The exact closed-world catalog exposed to a model or a future HTTP adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolName {
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

impl AgentToolName {
    pub const ALL: [Self; 14] = [
        Self::GetProjectProtocol,
        Self::GetReport,
        Self::ReadDocumentBlocks,
        Self::SearchDocument,
        Self::SearchProjectReports,
        Self::GetScreeningState,
        Self::GetStudy,
        Self::GetAppraisal,
        Self::ProposeScreeningDecision,
        Self::ProposeDuplicateMerge,
        Self::ProposeStudyGrouping,
        Self::ProposeClassification,
        Self::ProposeExtraction,
        Self::ProposeAppraisalAnswer,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GetProjectProtocol => "get_project_protocol",
            Self::GetReport => "get_report",
            Self::ReadDocumentBlocks => "read_document_blocks",
            Self::SearchDocument => "search_document",
            Self::SearchProjectReports => "search_project_reports",
            Self::GetScreeningState => "get_screening_state",
            Self::GetStudy => "get_study",
            Self::GetAppraisal => "get_appraisal",
            Self::ProposeScreeningDecision => "propose_screening_decision",
            Self::ProposeDuplicateMerge => "propose_duplicate_merge",
            Self::ProposeStudyGrouping => "propose_study_grouping",
            Self::ProposeClassification => "propose_classification",
            Self::ProposeExtraction => "propose_extraction",
            Self::ProposeAppraisalAnswer => "propose_appraisal_answer",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|name| name.as_str() == value)
    }

    pub const fn is_read(self) -> bool {
        matches!(
            self,
            Self::GetProjectProtocol
                | Self::GetReport
                | Self::ReadDocumentBlocks
                | Self::SearchDocument
                | Self::SearchProjectReports
                | Self::GetScreeningState
                | Self::GetStudy
                | Self::GetAppraisal
        )
    }

    pub const fn is_proposal(self) -> bool {
        !self.is_read()
    }

    pub const fn policy(self) -> AgentToolPolicy {
        match self {
            Self::GetProjectProtocol
            | Self::GetReport
            | Self::ReadDocumentBlocks
            | Self::SearchDocument
            | Self::SearchProjectReports
            | Self::GetScreeningState
            | Self::GetStudy
            | Self::GetAppraisal => AgentToolPolicy {
                action: RequestedAction::Read,
                authority: AuthorityTier::ReadOnly,
            },
            Self::ProposeDuplicateMerge
            | Self::ProposeStudyGrouping
            | Self::ProposeClassification => AgentToolPolicy {
                action: RequestedAction::WorkflowSuggestion,
                authority: AuthorityTier::WorkflowSuggestion,
            },
            Self::ProposeScreeningDecision
            | Self::ProposeExtraction
            | Self::ProposeAppraisalAnswer => AgentToolPolicy {
                action: RequestedAction::ScientificConclusion,
                authority: AuthorityTier::ScientificConclusion,
            },
        }
    }
}

impl fmt::Display for AgentToolName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Fixed policy metadata for one catalog entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentToolPolicy {
    pub action: RequestedAction,
    pub authority: AuthorityTier,
}

/// Project-only arguments shared by the protocol read tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectToolArgs {
    pub project_id: ProjectId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportToolArgs {
    pub project_id: ProjectId,
    pub report_id: ReportId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentBlocksToolArgs {
    pub project_id: ProjectId,
    pub document_id: DocumentId,
    pub block_ids: Vec<DocumentBlockId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchDocumentToolArgs {
    pub project_id: ProjectId,
    pub document_id: DocumentId,
    pub query: String,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchProjectReportsToolArgs {
    pub project_id: ProjectId,
    pub query: String,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreeningStateToolArgs {
    pub project_id: ProjectId,
    pub report_id: ReportId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StudyToolArgs {
    pub project_id: ProjectId,
    pub study_id: StudyId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppraisalToolArgs {
    pub project_id: ProjectId,
    pub report_id: ReportId,
    pub definition_id: String,
    pub definition_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreeningDecisionProposalArgs {
    pub project_id: ProjectId,
    pub report_id: ReportId,
    pub stage: ScreeningStage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DuplicateMergeProposalArgs {
    pub project_id: ProjectId,
    pub source_record_id: RecordId,
    pub candidate_report_id: ReportId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StudyGroupingProposalArgs {
    pub project_id: ProjectId,
    pub report_id: ReportId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassificationProposalArgs {
    pub project_id: ProjectId,
    pub study_id: StudyId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractionProposalArgs {
    pub project_id: ProjectId,
    pub study_id: StudyId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppraisalAnswerProposalArgs {
    pub project_id: ProjectId,
    pub report_id: ReportId,
    pub definition_id: String,
    pub definition_version: u32,
}

/// A typed request accepted by the agent runtime.
///
/// The custom deserializer rejects unknown top-level fields as well as unknown
/// tool names.  Every argument object also denies unknown fields, so a caller
/// cannot smuggle an action, SQL statement, or policy override through JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "tool", content = "args", rename_all = "snake_case")]
pub enum AgentTool {
    GetProjectProtocol(ProjectToolArgs),
    GetReport(ReportToolArgs),
    ReadDocumentBlocks(DocumentBlocksToolArgs),
    SearchDocument(SearchDocumentToolArgs),
    SearchProjectReports(SearchProjectReportsToolArgs),
    GetScreeningState(ScreeningStateToolArgs),
    GetStudy(StudyToolArgs),
    GetAppraisal(AppraisalToolArgs),
    ProposeScreeningDecision(ScreeningDecisionProposalArgs),
    ProposeDuplicateMerge(DuplicateMergeProposalArgs),
    ProposeStudyGrouping(StudyGroupingProposalArgs),
    ProposeClassification(ClassificationProposalArgs),
    ProposeExtraction(ExtractionProposalArgs),
    ProposeAppraisalAnswer(AppraisalAnswerProposalArgs),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireAgentTool {
    tool: AgentToolName,
    args: Value,
}

impl<'de> Deserialize<'de> for AgentTool {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = WireAgentTool::deserialize(deserializer)?;
        Self::from_name_and_args(wire.tool, wire.args).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AgentToolParseError {
    #[error("agent tool request is malformed")]
    MalformedRequest,
    #[error("agent tool is not in the allowlist")]
    UnknownTool,
}

impl AgentTool {
    pub const fn name(&self) -> AgentToolName {
        match self {
            Self::GetProjectProtocol(_) => AgentToolName::GetProjectProtocol,
            Self::GetReport(_) => AgentToolName::GetReport,
            Self::ReadDocumentBlocks(_) => AgentToolName::ReadDocumentBlocks,
            Self::SearchDocument(_) => AgentToolName::SearchDocument,
            Self::SearchProjectReports(_) => AgentToolName::SearchProjectReports,
            Self::GetScreeningState(_) => AgentToolName::GetScreeningState,
            Self::GetStudy(_) => AgentToolName::GetStudy,
            Self::GetAppraisal(_) => AgentToolName::GetAppraisal,
            Self::ProposeScreeningDecision(_) => AgentToolName::ProposeScreeningDecision,
            Self::ProposeDuplicateMerge(_) => AgentToolName::ProposeDuplicateMerge,
            Self::ProposeStudyGrouping(_) => AgentToolName::ProposeStudyGrouping,
            Self::ProposeClassification(_) => AgentToolName::ProposeClassification,
            Self::ProposeExtraction(_) => AgentToolName::ProposeExtraction,
            Self::ProposeAppraisalAnswer(_) => AgentToolName::ProposeAppraisalAnswer,
        }
    }

    pub const fn project_id(&self) -> ProjectId {
        match self {
            Self::GetProjectProtocol(args) => args.project_id,
            Self::GetReport(args) => args.project_id,
            Self::ReadDocumentBlocks(args) => args.project_id,
            Self::SearchDocument(args) => args.project_id,
            Self::SearchProjectReports(args) => args.project_id,
            Self::GetScreeningState(args) => args.project_id,
            Self::GetStudy(args) => args.project_id,
            Self::GetAppraisal(args) => args.project_id,
            Self::ProposeScreeningDecision(args) => args.project_id,
            Self::ProposeDuplicateMerge(args) => args.project_id,
            Self::ProposeStudyGrouping(args) => args.project_id,
            Self::ProposeClassification(args) => args.project_id,
            Self::ProposeExtraction(args) => args.project_id,
            Self::ProposeAppraisalAnswer(args) => args.project_id,
        }
    }

    pub const fn policy(&self) -> AgentToolPolicy {
        self.name().policy()
    }

    /// Parse an HTTP/model JSON envelope into the closed-world typed request.
    pub fn parse_json(input: &str) -> Result<Self, AgentToolParseError> {
        let raw: Value =
            serde_json::from_str(input).map_err(|_| AgentToolParseError::MalformedRequest)?;
        let object = raw
            .as_object()
            .ok_or(AgentToolParseError::MalformedRequest)?;
        if object.len() != 2 || !object.contains_key("tool") || !object.contains_key("args") {
            return Err(AgentToolParseError::MalformedRequest);
        }
        let tool = object
            .get("tool")
            .and_then(Value::as_str)
            .ok_or(AgentToolParseError::MalformedRequest)?;
        if AgentToolName::parse(tool).is_none() {
            return Err(AgentToolParseError::UnknownTool);
        }
        serde_json::from_value(raw).map_err(|_| AgentToolParseError::MalformedRequest)
    }

    pub fn validate(&self) -> Result<(), AgentToolError> {
        if self.project_id().as_uuid().is_nil() {
            return Err(AgentToolError::InvalidProjectScope);
        }
        match self {
            Self::GetProjectProtocol(_) => {}
            Self::GetReport(args) => validate_uuid(args.report_id.as_uuid())?,
            Self::ReadDocumentBlocks(args) => {
                validate_uuid(args.document_id.as_uuid())?;
                if args.block_ids.is_empty() || args.block_ids.len() > MAX_DOCUMENT_BLOCKS {
                    return Err(AgentToolError::InvalidArguments);
                }
                if args.block_ids.iter().any(|id| id.as_uuid().is_nil()) {
                    return Err(AgentToolError::InvalidArguments);
                }
            }
            Self::SearchDocument(args) => {
                validate_uuid(args.document_id.as_uuid())?;
                validate_search(&args.query, args.limit)?;
            }
            Self::SearchProjectReports(args) => validate_search(&args.query, args.limit)?,
            Self::GetScreeningState(args) => validate_uuid(args.report_id.as_uuid())?,
            Self::GetStudy(args) => validate_uuid(args.study_id.as_uuid())?,
            Self::GetAppraisal(args) => {
                validate_uuid(args.report_id.as_uuid())?;
                validate_definition(&args.definition_id, args.definition_version)?;
            }
            Self::ProposeScreeningDecision(args) => validate_uuid(args.report_id.as_uuid())?,
            Self::ProposeDuplicateMerge(args) => {
                validate_uuid(args.source_record_id.as_uuid())?;
                validate_uuid(args.candidate_report_id.as_uuid())?;
            }
            Self::ProposeStudyGrouping(args) => validate_uuid(args.report_id.as_uuid())?,
            Self::ProposeClassification(args) => validate_uuid(args.study_id.as_uuid())?,
            Self::ProposeExtraction(args) => validate_uuid(args.study_id.as_uuid())?,
            Self::ProposeAppraisalAnswer(args) => {
                validate_uuid(args.report_id.as_uuid())?;
                validate_definition(&args.definition_id, args.definition_version)?;
            }
        }
        Ok(())
    }

    fn from_name_and_args(name: AgentToolName, args: Value) -> Result<Self, &'static str> {
        fn decode<T: DeserializeOwned>(args: Value) -> Result<T, &'static str> {
            serde_json::from_value(args).map_err(|_| "agent tool arguments are malformed")
        }

        Ok(match name {
            AgentToolName::GetProjectProtocol => Self::GetProjectProtocol(decode(args)?),
            AgentToolName::GetReport => Self::GetReport(decode(args)?),
            AgentToolName::ReadDocumentBlocks => Self::ReadDocumentBlocks(decode(args)?),
            AgentToolName::SearchDocument => Self::SearchDocument(decode(args)?),
            AgentToolName::SearchProjectReports => Self::SearchProjectReports(decode(args)?),
            AgentToolName::GetScreeningState => Self::GetScreeningState(decode(args)?),
            AgentToolName::GetStudy => Self::GetStudy(decode(args)?),
            AgentToolName::GetAppraisal => Self::GetAppraisal(decode(args)?),
            AgentToolName::ProposeScreeningDecision => {
                Self::ProposeScreeningDecision(decode(args)?)
            }
            AgentToolName::ProposeDuplicateMerge => Self::ProposeDuplicateMerge(decode(args)?),
            AgentToolName::ProposeStudyGrouping => Self::ProposeStudyGrouping(decode(args)?),
            AgentToolName::ProposeClassification => Self::ProposeClassification(decode(args)?),
            AgentToolName::ProposeExtraction => Self::ProposeExtraction(decode(args)?),
            AgentToolName::ProposeAppraisalAnswer => Self::ProposeAppraisalAnswer(decode(args)?),
        })
    }

    fn into_read_operation(self) -> Result<AgentReadOperation, AgentToolError> {
        match self {
            Self::GetProjectProtocol(args) => Ok(AgentReadOperation::GetProjectProtocol(args)),
            Self::GetReport(args) => Ok(AgentReadOperation::GetReport(args)),
            Self::ReadDocumentBlocks(args) => Ok(AgentReadOperation::ReadDocumentBlocks(args)),
            Self::SearchDocument(args) => Ok(AgentReadOperation::SearchDocument(args)),
            Self::SearchProjectReports(args) => Ok(AgentReadOperation::SearchProjectReports(args)),
            Self::GetScreeningState(args) => Ok(AgentReadOperation::GetScreeningState(args)),
            Self::GetStudy(args) => Ok(AgentReadOperation::GetStudy(args)),
            Self::GetAppraisal(args) => Ok(AgentReadOperation::GetAppraisal(args)),
            Self::ProposeScreeningDecision(_)
            | Self::ProposeDuplicateMerge(_)
            | Self::ProposeStudyGrouping(_)
            | Self::ProposeClassification(_)
            | Self::ProposeExtraction(_)
            | Self::ProposeAppraisalAnswer(_) => Err(AgentToolError::Forbidden),
        }
    }

    fn into_proposal_operation(self) -> Result<AgentProposalOperation, AgentToolError> {
        match self {
            Self::ProposeScreeningDecision(args) => {
                Ok(AgentProposalOperation::ProposeScreeningDecision(args))
            }
            Self::ProposeDuplicateMerge(args) => {
                Ok(AgentProposalOperation::ProposeDuplicateMerge(args))
            }
            Self::ProposeStudyGrouping(args) => {
                Ok(AgentProposalOperation::ProposeStudyGrouping(args))
            }
            Self::ProposeClassification(args) => {
                Ok(AgentProposalOperation::ProposeClassification(args))
            }
            Self::ProposeExtraction(args) => Ok(AgentProposalOperation::ProposeExtraction(args)),
            Self::ProposeAppraisalAnswer(args) => {
                Ok(AgentProposalOperation::ProposeAppraisalAnswer(args))
            }
            Self::GetProjectProtocol(_)
            | Self::GetReport(_)
            | Self::ReadDocumentBlocks(_)
            | Self::SearchDocument(_)
            | Self::SearchProjectReports(_)
            | Self::GetScreeningState(_)
            | Self::GetStudy(_)
            | Self::GetAppraisal(_) => Err(AgentToolError::Forbidden),
        }
    }
}

fn validate_search(query: &str, limit: u16) -> Result<(), AgentToolError> {
    validate_bounded_text(query, MAX_QUERY_LENGTH)?;
    if limit == 0 || limit > 100 {
        return Err(AgentToolError::InvalidArguments);
    }
    Ok(())
}

fn validate_definition(id: &str, version: u32) -> Result<(), AgentToolError> {
    validate_bounded_text(id, MAX_DEFINITION_ID_LENGTH)?;
    if version == 0 {
        return Err(AgentToolError::InvalidArguments);
    }
    Ok(())
}

fn validate_bounded_text(value: &str, max_length: usize) -> Result<(), AgentToolError> {
    if value.trim().is_empty() || value.chars().count() > max_length {
        Err(AgentToolError::InvalidArguments)
    } else {
        Ok(())
    }
}

fn validate_uuid(value: Uuid) -> Result<(), AgentToolError> {
    if value.is_nil() {
        Err(AgentToolError::InvalidArguments)
    } else {
        Ok(())
    }
}

/// The operation types the executor is allowed to receive for reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentReadOperation {
    GetProjectProtocol(ProjectToolArgs),
    GetReport(ReportToolArgs),
    ReadDocumentBlocks(DocumentBlocksToolArgs),
    SearchDocument(SearchDocumentToolArgs),
    SearchProjectReports(SearchProjectReportsToolArgs),
    GetScreeningState(ScreeningStateToolArgs),
    GetStudy(StudyToolArgs),
    GetAppraisal(AppraisalToolArgs),
}

impl AgentReadOperation {
    pub const fn name(&self) -> AgentToolName {
        match self {
            Self::GetProjectProtocol(_) => AgentToolName::GetProjectProtocol,
            Self::GetReport(_) => AgentToolName::GetReport,
            Self::ReadDocumentBlocks(_) => AgentToolName::ReadDocumentBlocks,
            Self::SearchDocument(_) => AgentToolName::SearchDocument,
            Self::SearchProjectReports(_) => AgentToolName::SearchProjectReports,
            Self::GetScreeningState(_) => AgentToolName::GetScreeningState,
            Self::GetStudy(_) => AgentToolName::GetStudy,
            Self::GetAppraisal(_) => AgentToolName::GetAppraisal,
        }
    }
}

/// The operation types the executor is allowed to receive for proposals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentProposalOperation {
    ProposeScreeningDecision(ScreeningDecisionProposalArgs),
    ProposeDuplicateMerge(DuplicateMergeProposalArgs),
    ProposeStudyGrouping(StudyGroupingProposalArgs),
    ProposeClassification(ClassificationProposalArgs),
    ProposeExtraction(ExtractionProposalArgs),
    ProposeAppraisalAnswer(AppraisalAnswerProposalArgs),
}

impl AgentProposalOperation {
    pub const fn name(&self) -> AgentToolName {
        match self {
            Self::ProposeScreeningDecision(_) => AgentToolName::ProposeScreeningDecision,
            Self::ProposeDuplicateMerge(_) => AgentToolName::ProposeDuplicateMerge,
            Self::ProposeStudyGrouping(_) => AgentToolName::ProposeStudyGrouping,
            Self::ProposeClassification(_) => AgentToolName::ProposeClassification,
            Self::ProposeExtraction(_) => AgentToolName::ProposeExtraction,
            Self::ProposeAppraisalAnswer(_) => AgentToolName::ProposeAppraisalAnswer,
        }
    }
}

/// JSON output returned by an application query, bounded before it crosses the
/// executor boundary.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoundedAgentJson(Value);

impl BoundedAgentJson {
    pub fn new(value: Value) -> Result<Self, AgentToolError> {
        let encoded = serde_json::to_vec(&value).map_err(|_| AgentToolError::InvalidOutput)?;
        if encoded.len() > MAX_OUTPUT_BYTES {
            return Err(AgentToolError::InvalidOutput);
        }
        Ok(Self(value))
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    pub fn into_value(self) -> Value {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AgentProposalReceipt {
    pub review_run_id: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("agent tool execution failed")]
pub struct AgentToolExecutionError;

pub type AgentReadFuture<'a> =
    Pin<Box<dyn Future<Output = Result<BoundedAgentJson, AgentToolExecutionError>> + Send + 'a>>;
pub type AgentProposalFuture<'a> = Pin<
    Box<dyn Future<Output = Result<AgentProposalReceipt, AgentToolExecutionError>> + Send + 'a>,
>;

/// Application-service port used after policy authorization.
pub trait AgentToolExecutor: Send + Sync {
    fn execute_read<'a>(&'a self, operation: AgentReadOperation) -> AgentReadFuture<'a>;
    fn create_proposal<'a>(&'a self, operation: AgentProposalOperation) -> AgentProposalFuture<'a>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AgentToolError {
    #[error("agent tool request is malformed")]
    MalformedRequest,
    #[error("agent tool is not in the allowlist")]
    UnknownTool,
    #[error("agent actor is invalid")]
    InvalidActor,
    #[error("agent project scope is invalid")]
    InvalidProjectScope,
    #[error("agent tool arguments are invalid")]
    InvalidArguments,
    #[error("agent tool is forbidden")]
    Forbidden,
    #[error("agent tool output is invalid or too large")]
    InvalidOutput,
    #[error("agent tool execution failed")]
    ExecutionFailed,
}

/// Project-scoped dispatcher.  It performs all boundary validation and policy
/// checks before invoking the application-service executor.
#[derive(Debug, Clone)]
pub struct AgentRuntime {
    project_id: ProjectId,
    project_policy: ProjectAiPolicy,
    policy_engine: PolicyEngine,
}

impl AgentRuntime {
    pub const fn new(project_id: ProjectId, project_policy: ProjectAiPolicy) -> Self {
        Self {
            project_id,
            project_policy,
            policy_engine: PolicyEngine,
        }
    }

    pub const fn project_id(&self) -> ProjectId {
        self.project_id
    }

    pub fn dispatch<'a, E: AgentToolExecutor + 'a>(
        &self,
        actor: &Actor,
        tool: AgentTool,
        executor: &'a E,
    ) -> Result<AgentDispatch<'a>, AgentToolError> {
        tool.validate()?;
        if self.project_id.as_uuid().is_nil() {
            return Err(AgentToolError::InvalidProjectScope);
        }
        if actor.id().trim().is_empty() {
            return Err(AgentToolError::InvalidActor);
        }

        let metadata = tool.policy();
        let input = PolicyInput {
            actor: actor.clone(),
            project_id: self.project_id,
            declared_project_id: tool.project_id(),
            tool: tool.name().as_str().to_owned(),
            action: metadata.action,
            authority: metadata.authority,
            args: serde_json::to_value(&tool).map_err(|_| AgentToolError::MalformedRequest)?,
            project_policy: self.project_policy.clone(),
        };

        match self.policy_engine.authorize(&input) {
            PolicyDecision::ExecuteRead => {
                let operation = tool.into_read_operation()?;
                Ok(AgentDispatch::Read(executor.execute_read(operation)))
            }
            PolicyDecision::CreateProposal => {
                let operation = tool.into_proposal_operation()?;
                Ok(AgentDispatch::Proposal(executor.create_proposal(operation)))
            }
            PolicyDecision::ExecuteReversibleWrite | PolicyDecision::Forbidden => {
                if self.project_id != input.declared_project_id {
                    Err(AgentToolError::InvalidProjectScope)
                } else {
                    Err(AgentToolError::Forbidden)
                }
            }
        }
    }
}

/// The future selected by the policy decision.  No executor call is made for
/// the error variants returned by [`AgentRuntime::dispatch`].
pub enum AgentDispatch<'a> {
    Read(AgentReadFuture<'a>),
    Proposal(AgentProposalFuture<'a>),
}
