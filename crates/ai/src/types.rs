use std::{collections::BTreeMap, fmt};

use chrono::{DateTime, Utc};
use deepref_domain::{DocumentBlockId, DocumentId, ProjectId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

pub type AiFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, AiError>> + Send + 'a>>;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum AiError {
    #[error("AI input could not be serialized")]
    InputSerialization(String),
    #[error("AI context is invalid")]
    InvalidContext(String),
    #[error("AI route could not be resolved")]
    Route(String),
    #[error("AI provider failed")]
    Gateway(String),
    #[error("AI output was not valid JSON")]
    MalformedOutput(String),
    #[error("AI output failed JSON Schema validation")]
    SchemaValidation(String),
    #[error("AI output failed semantic validation")]
    SemanticValidation(String),
    #[error("AI persistence failed")]
    Persistence(String),
    #[error("AI proposal failed")]
    Proposal(String),
    #[error("AI prompt registry rejected the definition")]
    PromptRegistry(String),
    #[error("AI embedding is invalid")]
    InvalidEmbedding(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AiTaskKind {
    MetadataCleanup,
    LanguageDetection,
    StudyDesignClassification,
    PicoExtraction,
    DuplicateCandidateDetection,
    TitleAbstractScreening,
    FullTextScreening,
    StudyGrouping,
    AppraisalPrefill,
    DataExtraction,
    Synthesis,
}

impl AiTaskKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MetadataCleanup => "metadata_cleanup",
            Self::LanguageDetection => "language_detection",
            Self::StudyDesignClassification => "study_design_classification",
            Self::PicoExtraction => "pico_extraction",
            Self::DuplicateCandidateDetection => "duplicate_candidate_detection",
            Self::TitleAbstractScreening => "title_abstract_screening",
            Self::FullTextScreening => "full_text_screening",
            Self::StudyGrouping => "study_grouping",
            Self::AppraisalPrefill => "appraisal_prefill",
            Self::DataExtraction => "data_extraction",
            Self::Synthesis => "synthesis",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [
            Self::MetadataCleanup,
            Self::LanguageDetection,
            Self::StudyDesignClassification,
            Self::PicoExtraction,
            Self::DuplicateCandidateDetection,
            Self::TitleAbstractScreening,
            Self::FullTextScreening,
            Self::StudyGrouping,
            Self::AppraisalPrefill,
            Self::DataExtraction,
            Self::Synthesis,
        ]
        .into_iter()
        .find(|kind| kind.as_str() == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelProfile {
    FastClassifier,
    Embedding,
    Reasoning,
    LongContextReasoning,
    PremiumSynthesis,
}

impl ModelProfile {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FastClassifier => "fast_classifier",
            Self::Embedding => "embedding",
            Self::Reasoning => "reasoning",
            Self::LongContextReasoning => "long_context_reasoning",
            Self::PremiumSynthesis => "premium_synthesis",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [
            Self::FastClassifier,
            Self::Embedding,
            Self::Reasoning,
            Self::LongContextReasoning,
            Self::PremiumSynthesis,
        ]
        .into_iter()
        .find(|profile| profile.as_str() == value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModelParameters {
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f32>,
    #[serde(default)]
    pub additional: BTreeMap<String, Value>,
}

impl Default for ModelParameters {
    fn default() -> Self {
        Self {
            temperature: Some(0.0),
            max_tokens: None,
            top_p: None,
            additional: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedModel {
    pub profile: ModelProfile,
    pub provider: String,
    pub model: String,
    pub model_version: String,
    pub parameters: ModelParameters,
    pub route_id: Option<Uuid>,
}

impl ResolvedModel {
    pub fn validate(&self) -> Result<(), AiError> {
        if self.provider.trim().is_empty()
            || self.model.trim().is_empty()
            || self.model_version.trim().is_empty()
        {
            return Err(AiError::Route("route identity is incomplete".to_owned()));
        }
        if self
            .parameters
            .temperature
            .is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value))
        {
            return Err(AiError::Route("temperature is outside 0..=2".to_owned()));
        }
        if self
            .parameters
            .top_p
            .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
        {
            return Err(AiError::Route("top_p is outside 0..=1".to_owned()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Embedding(Vec<f32>);

impl Embedding {
    pub fn new(values: Vec<f32>) -> Result<Self, AiError> {
        if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
            return Err(AiError::InvalidEmbedding(
                "embedding is not finite".to_owned(),
            ));
        }
        Ok(Self(values))
    }
    pub fn dimension(&self) -> usize {
        self.0.len()
    }
    pub fn as_slice(&self) -> &[f32] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub document_block_id: DocumentBlockId,
    pub page: u32,
    pub content_hash: String,
    #[serde(default)]
    pub section_path: Vec<String>,
    #[serde(default)]
    pub retrieval_rank: u32,
    #[serde(default)]
    pub retrieval_score: f64,
}

impl EvidenceRef {
    pub fn new(
        document_block_id: DocumentBlockId,
        page: u32,
        content_hash: impl Into<String>,
    ) -> Result<Self, AiError> {
        let content_hash = content_hash.into();
        if page == 0 || !is_sha256(&content_hash) {
            return Err(AiError::InvalidContext(
                "evidence identity is invalid".to_owned(),
            ));
        }
        Ok(Self {
            document_block_id,
            page,
            content_hash,
            section_path: Vec::new(),
            retrieval_rank: 0,
            retrieval_score: 0.0,
        })
    }
    pub fn with_section_path(mut self, section_path: Vec<String>) -> Self {
        self.section_path = section_path;
        self
    }
    pub fn with_retrieval(mut self, rank: u32, score: f64) -> Result<Self, AiError> {
        if rank == 0 || !score.is_finite() || score < 0.0 {
            return Err(AiError::InvalidContext(
                "retrieval metadata is invalid".to_owned(),
            ));
        }
        self.retrieval_rank = rank;
        self.retrieval_score = score;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GroundedBlock {
    pub evidence: EvidenceRef,
    pub text: String,
    pub retrieval_rank: u32,
    pub retrieval_score: f64,
}

impl GroundedBlock {
    pub fn validate(&self) -> Result<(), AiError> {
        if self.text.is_empty()
            || self.retrieval_rank == 0
            || !self.retrieval_score.is_finite()
            || self.retrieval_score < 0.0
        {
            return Err(AiError::InvalidContext(
                "grounded evidence is invalid".to_owned(),
            ));
        }
        if self.evidence.retrieval_rank != self.retrieval_rank
            || (self.evidence.retrieval_score - self.retrieval_score).abs() > f64::EPSILON
        {
            return Err(AiError::InvalidContext(
                "grounding metadata disagrees".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalRequest {
    pub project_id: ProjectId,
    pub report_id: Option<Uuid>,
    pub document_id: Option<DocumentId>,
    pub query: String,
    pub embedding: Option<Embedding>,
    pub section_prefix: Option<Vec<String>>,
    pub kind: Option<String>,
    pub limit: u32,
}

impl RetrievalRequest {
    pub fn validate(&self) -> Result<(), AiError> {
        if self.query.trim().is_empty() && self.embedding.is_none() {
            return Err(AiError::InvalidContext(
                "retrieval query is empty".to_owned(),
            ));
        }
        if self.limit == 0 || self.limit > 100 {
            return Err(AiError::InvalidContext(
                "retrieval limit is outside 1..=100".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AiContext {
    pub project_id: Option<ProjectId>,
    pub system_prompt: String,
    pub user_prompt: String,
    pub retrieval: Option<RetrievalRequest>,
    pub protocol_hash: Option<String>,
    pub document_hash: Option<String>,
}

impl AiContext {
    pub fn validate(&self) -> Result<(), AiError> {
        if self.system_prompt.trim().is_empty() || self.user_prompt.trim().is_empty() {
            return Err(AiError::InvalidContext(
                "AI prompts must not be blank".to_owned(),
            ));
        }
        if let Some(retrieval) = &self.retrieval {
            retrieval.validate()?;
        }
        for hash in [&self.protocol_hash, &self.document_hash]
            .into_iter()
            .flatten()
        {
            if !is_sha256(hash) {
                return Err(AiError::InvalidContext(
                    "context hash is invalid".to_owned(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompletionRequest {
    pub route: ResolvedModel,
    pub system_prompt: String,
    pub user_prompt: String,
    pub evidence: Vec<GroundedBlock>,
    pub schema: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayCompletion {
    pub output_json: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_micros: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AiRunStatus {
    Running,
    Completed,
    Failed,
    Abstained,
}

impl AiRunStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Abstained => "abstained",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        [
            Self::Running,
            Self::Completed,
            Self::Failed,
            Self::Abstained,
        ]
        .into_iter()
        .find(|status| status.as_str() == value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SafeErrorMetadata {
    pub code: String,
    pub message: String,
}

impl SafeErrorMetadata {
    fn is_safe(&self) -> bool {
        !self.code.trim().is_empty()
            && self.code.len() <= 64
            && self
                .code
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            && !self.message.trim().is_empty()
            && self.message.len() <= 128
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AiRunRecord {
    pub id: Uuid,
    pub project_id: Option<ProjectId>,
    pub task_kind: AiTaskKind,
    pub route: ResolvedModel,
    pub prompt_version: String,
    pub prompt_hash: String,
    pub schema_version: String,
    pub schema_hash: String,
    pub input_hash: String,
    pub reuse_hash: String,
    pub protocol_hash: Option<String>,
    pub document_hash: Option<String>,
    pub evidence_hash: Option<String>,
    pub evidence_refs: Vec<EvidenceRef>,
    pub usage: TokenUsage,
    pub cost_micros: Option<i64>,
    pub output: Option<Value>,
    pub status: AiRunStatus,
    pub error: Option<SafeErrorMetadata>,
    pub parent_automation_run_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl AiRunRecord {
    pub fn validate(&self) -> Result<(), AiError> {
        let valid = match self.status {
            AiRunStatus::Running => {
                self.completed_at.is_none() && self.output.is_none() && self.error.is_none()
            }
            AiRunStatus::Completed => {
                self.completed_at.is_some() && self.output.is_some() && self.error.is_none()
            }
            AiRunStatus::Failed | AiRunStatus::Abstained => {
                self.completed_at.is_some()
                    && self.output.is_none()
                    && self.error.as_ref().is_some_and(SafeErrorMetadata::is_safe)
            }
        };
        if valid {
            Ok(())
        } else {
            Err(AiError::Persistence(
                "AI run status has an invalid completion shape".to_owned(),
            ))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityTier {
    ReadOnly,
    ReversibleMetadata,
    WorkflowSuggestion,
    ScientificConclusion,
}

impl AuthorityTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::ReversibleMetadata => "reversible_metadata",
            Self::WorkflowSuggestion => "workflow_suggestion",
            Self::ScientificConclusion => "scientific_conclusion",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        [
            Self::ReadOnly,
            Self::ReversibleMetadata,
            Self::WorkflowSuggestion,
            Self::ScientificConclusion,
        ]
        .into_iter()
        .find(|tier| tier.as_str() == value)
    }
    pub const fn requires_proposal(self) -> bool {
        matches!(self, Self::WorkflowSuggestion | Self::ScientificConclusion)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Pending,
    Accepted,
    Rejected,
    Expired,
}
impl ProposalStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProposalDraft {
    pub project_id: ProjectId,
    pub entity_type: String,
    pub entity_id: Option<Uuid>,
    pub operation: String,
    pub payload: Value,
    pub authority: AuthorityTier,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AiProposal {
    pub id: Uuid,
    pub draft: ProposalDraft,
    pub model_run_id: Uuid,
    pub status: ProposalStatus,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by_actor_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReuseKeyInput {
    pub task_kind: String,
    pub provider: String,
    pub model: String,
    pub model_version: String,
    pub parameters: Value,
    pub prompt_version: String,
    pub prompt_hash: String,
    pub schema_version: String,
    pub schema_hash: String,
    pub input_hash: String,
    pub protocol_hash: Option<String>,
    pub document_hash: Option<String>,
    pub evidence_hash: Option<String>,
}

pub fn compute_reuse_hash(input: &ReuseKeyInput) -> Result<String, AiError> {
    hash_json(
        &serde_json::to_value(input)
            .map_err(|_| AiError::InputSerialization("reuse key".to_owned()))?,
    )
}

pub fn hash_json(value: &Value) -> Result<String, AiError> {
    let bytes = serde_json::to_vec(&canonicalize(value))
        .map_err(|_| AiError::InputSerialization("canonical JSON".to_owned()))?;
    Ok(sha256_bytes(&bytes))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), canonicalize(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(canonicalize).collect()),
        other => other.clone(),
    }
}

impl fmt::Display for ReuseKeyInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.task_kind)
    }
}
