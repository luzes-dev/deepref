use super::*;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AiScreeningStageInput {
    TitleAbstract,
    FullText,
}

impl AiScreeningStageInput {
    pub(super) fn ai(self) -> ScreeningStage {
        match self {
            Self::TitleAbstract => ScreeningStage::TitleAbstract,
            Self::FullText => ScreeningStage::FullText,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct GenerateScreeningRequest {
    pub stage: AiScreeningStageInput,
    pub protocol_version_id: Option<Uuid>,
    pub expected_revision: Option<i64>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct GenerateDuplicateRequest {
    pub candidate_report_id: Uuid,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct GenerateAppraisalPrefillRequest {
    pub definition_id: String,
    pub definition_version: u32,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct ReviewRunDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub definition: String,
    pub subject: Value,
    pub origin: Value,
    pub state: ReviewRunStateDto,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ReviewRunStateDto {
    Queued,
    Running,
    Blocked { code: String, message: String },
    Failed { code: String, message: String },
    Completed { proposal_id: Uuid },
}

pub(crate) type AcceptedReviewRun = (axum::http::StatusCode, HeaderMap, Json<ReviewRunDto>);

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AiProposalDecisionInput {
    Accept,
    Reject,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct DecideAiProposalRequest {
    pub decision: AiProposalDecisionInput,
    pub reason: String,
    #[serde(default)]
    #[schema(required = false)]
    pub reviewed_payload: Option<AiReviewedProposalPayload>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AiReviewedProposalPayload {
    AppraisalPrefill {
        report_id: Uuid,
        definition_id: String,
        definition_version: u32,
        answers: Vec<AiAppraisalPrefillAnswerDto>,
        domain_judgments: std::collections::BTreeMap<String, String>,
        overall_judgment: String,
    },
    DataExtraction {
        study_id: Uuid,
        fields: Vec<AiExtractedFieldDto>,
    },
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct AiProposalListParams {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub status: Option<String>,
    pub task_kind: Option<String>,
    pub target_report_id: Option<Uuid>,
    pub target_record_id: Option<Uuid>,
    pub candidate_report_id: Option<Uuid>,
    pub target_study_id: Option<Uuid>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AiProposalPayload {
    Screening(AiScreeningProposalPayload),
    Duplicate(AiDuplicateProposalPayload),
    StudyGrouping(AiStudyGroupingProposalPayload),
    Classification(AiStudyDesignClassificationProposalPayload),
    AppraisalPrefill(AiAppraisalPrefillProposalPayload),
    DataExtraction(AiDataExtractionProposalPayload),
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct AiStudyDesignClassificationProposalPayload {
    pub study_id: Uuid,
    pub suggested_design: Option<AiStudyDesignLabelDto>,
    pub rationale: String,
    pub evidence: Vec<AiStudyDesignEvidenceDto>,
    pub uncertainties: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AiStudyDesignLabelDto {
    Rct,
    NonRandomizedIntervention,
    Cohort,
    CaseControl,
    CrossSectional,
    DiagnosticAccuracy,
    PredictionModel,
    Qualitative,
    SystematicReview,
    CaseSeries,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AiStudyDesignEvidenceDto {
    StudyMetadata {
        study_id: Uuid,
        field: AiStudyMetadataFieldDto,
        content_hash: String,
    },
    ReportMetadata {
        report_id: Uuid,
        field: AiClassificationReportFieldDto,
        content_hash: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AiStudyMetadataFieldDto {
    Title,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AiClassificationReportFieldDto {
    Title,
    Abstract,
    PublicationYear,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct AiStudyGroupingProposalPayload {
    pub report_id: Uuid,
    pub expected_previous_study_id: Option<Uuid>,
    pub expected_previous_study_revision: Option<i64>,
    pub choice: AiStudyGroupingChoiceDto,
    pub rationale: String,
    pub provenance: Vec<AiStudyGroupingEvidenceDto>,
    pub uncertainties: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AiStudyGroupingChoiceDto {
    ExistingStudy {
        study_id: Uuid,
        expected_revision: i64,
    },
    NewStudy {
        title: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AiStudyGroupingFieldDto {
    Title,
    Abstract,
    PublicationYear,
    FirstAuthor,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(clippy::enum_variant_names)]
pub(crate) enum AiStudyGroupingEvidenceDto {
    ReportMetadata {
        report_id: Uuid,
        field: AiStudyGroupingFieldDto,
        content_hash: String,
    },
    StudyMetadata {
        study_id: Uuid,
        field: AiStudyGroupingFieldDto,
        content_hash: String,
    },
    StudyReportMetadata {
        study_id: Uuid,
        report_id: Uuid,
        field: AiStudyGroupingFieldDto,
        content_hash: String,
    },
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct AiAppraisalPrefillProposalPayload {
    pub report_id: Uuid,
    pub definition_id: String,
    pub definition_version: u32,
    pub answers: Vec<AiAppraisalPrefillAnswerDto>,
    pub domain_judgments: std::collections::BTreeMap<String, String>,
    pub overall_judgment: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct AiAppraisalPrefillAnswerDto {
    pub question_id: String,
    pub answer: AiAppraisalAnswerValueDto,
    pub rationale: String,
    pub evidence: Vec<AiAppraisalPrefillEvidenceDto>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AiAppraisalAnswerValueDto {
    Enum { value: String },
    Boolean { value: bool },
    Scale { value: i64 },
    Text { value: String },
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct AiAppraisalPrefillEvidenceDto {
    pub document_id: Uuid,
    pub document_block_id: Uuid,
    pub page: u32,
    pub parser_version: String,
    pub content_hash: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct AiDataExtractionProposalPayload {
    pub study_id: Uuid,
    pub fields: Vec<AiExtractedFieldDto>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AiExtractedFieldDto {
    Value {
        field_id: Uuid,
        field_version: u32,
        value: AiTypedExtractionValueDto,
        rationale: String,
        source: AiExtractionEvidenceDto,
    },
    InsufficientEvidence {
        field_id: Uuid,
        field_version: u32,
        rationale: String,
    },
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AiTypedExtractionValueDto {
    Text { value: String },
    Number { value: f64 },
    Boolean { value: bool },
    Date { value: String },
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct AiExtractionEvidenceDto {
    pub report_id: Uuid,
    pub document_id: Uuid,
    pub document_block_id: Uuid,
    pub page: u32,
    pub parser_version: String,
    pub content_hash: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct AiScreeningProposalPayload {
    pub task_kind: String,
    pub report_id: Uuid,
    pub expected_revision: i64,
    pub stage: AiScreeningStageInput,
    pub protocol_version_id: Uuid,
    pub criteria: Vec<AiCriterionJudgmentDto>,
    pub suggested_decision: AiSuggestedDecisionDto,
    pub uncertainties: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct AiCriterionJudgmentDto {
    pub criterion_id: Uuid,
    pub criterion_label: String,
    pub judgment: AiCriterionResultDto,
    pub rationale: String,
    pub evidence: Vec<AiScreeningEvidenceDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AiCriterionResultDto {
    Meets,
    DoesNotMeet,
    Unclear,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AiScreeningEvidenceDto {
    ReportMetadata {
        report_id: Uuid,
        field: AiScreeningEvidenceFieldDto,
        content_hash: String,
    },
    DocumentBlock {
        document_block_id: Uuid,
        page: u32,
        content_hash: String,
        section_path: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AiScreeningEvidenceFieldDto {
    Title,
    Abstract,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AiSuggestedDecisionDto {
    Include,
    Exclude { exclusion_reason_id: Option<Uuid> },
    Maybe,
    InsufficientEvidence,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct AiDuplicateProposalPayload {
    pub task_kind: String,
    pub candidate: AiDuplicateCandidateDto,
    pub decision: AiDuplicateDecisionDto,
    pub rationale: Vec<AiDuplicateRationaleDto>,
    pub signals: Vec<AiDuplicateSignalDto>,
    pub provenance: Vec<AiIdentityProvenanceDto>,
    pub uncertainties: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct AiDuplicateCandidateDto {
    pub source_record_id: Uuid,
    pub candidate_report_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AiDuplicateDecisionDto {
    Match,
    NoMatch,
    InsufficientEvidence,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct AiDuplicateRationaleDto {
    pub code: String,
    pub explanation: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AiDuplicateSignalDto {
    TitleSimilarity {
        similarity: f64,
        supports_match: bool,
    },
    PublicationYear {
        source_year: i32,
        candidate_year: i32,
        supports_match: bool,
    },
    FirstAuthor {
        source_author: String,
        candidate_author: String,
        similarity: f64,
        supports_match: bool,
    },
    DurableIdentifier {
        scheme: String,
        source_value: String,
        candidate_value: String,
        supports_match: bool,
    },
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub(crate) struct AiIdentityProvenanceDto {
    pub entity_type: String,
    pub entity_id: String,
    pub field: String,
    pub content_hash: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AiProposalDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub task_kind: String,
    pub entity_type: String,
    pub entity_id: Option<Uuid>,
    pub operation: String,
    pub payload: AiProposalPayload,
    pub authority_tier: String,
    pub model_run_id: Uuid,
    pub provider: String,
    pub model: String,
    pub model_version: String,
    pub prompt_version: String,
    pub schema_version: String,
    pub prompt_hash: String,
    pub schema_hash: String,
    pub input_hash: String,
    pub evidence_hash: Option<String>,
    pub status: String,
    pub protocol_version_id: Option<Uuid>,
    pub expected_revision: Option<i64>,
    pub target_report_id: Option<Uuid>,
    pub target_record_id: Option<Uuid>,
    pub target_study_id: Option<Uuid>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by_actor_kind: Option<String>,
    pub resolved_by_actor_id: Option<String>,
    pub resolution_reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AiProposalDecisionDto {
    pub proposal: AiProposalDto,
    pub applied_revision: Option<i64>,
}
