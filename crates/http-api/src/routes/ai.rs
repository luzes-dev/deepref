use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header::LOCATION},
};
use chrono::{DateTime, Utc};
use deepref_ai::{AiError, ScreeningStage};
use deepref_postgres::{
    AiProposalDecision, AiProposalDecisionRequest, AiProposalError, AiProposalRecord,
    ReviewedAiProposalPayload,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

use super::{
    pagination::{PaginatedResponse, PaginationParams, page},
    review::extract_actor,
};
use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AiScreeningStageInput {
    TitleAbstract,
    FullText,
}

impl AiScreeningStageInput {
    fn ai(self) -> ScreeningStage {
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

pub(super) type AcceptedReviewRun = (StatusCode, HeaderMap, Json<ReviewRunDto>);

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
    AppraisalPrefill(AiAppraisalPrefillProposalPayload),
    DataExtraction(AiDataExtractionProposalPayload),
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

#[utoipa::path(
    post,
    path = "/projects/{project_id}/reports/{report_id}/ai/screening",
    operation_id = "generateScreeningSuggestion",
    tag = "ai",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("report_id" = Uuid, Path, description = "Report identifier")
    ),
    request_body = GenerateScreeningRequest,
    responses(
        (status = 202, description = "Compiled screening review scheduled", body = ReviewRunDto),
        (status = 400, description = "Invalid AI request", body = ErrorResponse),
        (status = 404, description = "Project, report, or protocol not found", body = ErrorResponse),
        (status = 409, description = "A current proposal or revision conflicts", body = ErrorResponse),
        (status = 503, description = "AI provider is unavailable", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn generate_screening_suggestion(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(input): Json<GenerateScreeningRequest>,
) -> Result<AcceptedReviewRun, ApiError> {
    let snapshot = deepref_postgres::schedule_screening_review(
        &state.pool,
        project_id,
        report_id,
        input.stage.ai(),
        input.protocol_version_id,
        input.expected_revision,
        extract_actor(&headers)?,
    )
    .await
    .map_err(map_review_preparation_error)?;
    accepted_review_run(snapshot)
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/reports/{report_id}/ai/study-grouping",
    operation_id = "generateStudyGroupingSuggestion",
    tag = "ai",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path)),
    responses(
        (status = 202, body = ReviewRunDto),
        (status = 400, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 503, body = ErrorResponse),
        (status = 500, body = ErrorResponse)
    )
)]
pub(crate) async fn generate_study_grouping_suggestion(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<AcceptedReviewRun, ApiError> {
    let snapshot = deepref_postgres::schedule_study_grouping_review(
        &state.pool,
        project_id,
        report_id,
        extract_actor(&headers)?,
    )
    .await
    .map_err(map_review_preparation_error)?;
    accepted_review_run(snapshot)
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/reports/{report_id}/ai/appraisal-prefill",
    operation_id = "generateAppraisalPrefillSuggestion",
    tag = "ai",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path)),
    request_body = GenerateAppraisalPrefillRequest,
    responses(
        (status = 202, body = ReviewRunDto),
        (status = 400, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 503, body = ErrorResponse),
        (status = 500, body = ErrorResponse)
    )
)]
pub(crate) async fn generate_appraisal_prefill_suggestion(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(input): Json<GenerateAppraisalPrefillRequest>,
) -> Result<AcceptedReviewRun, ApiError> {
    let snapshot = deepref_postgres::schedule_appraisal_prefill_review(
        &state.pool,
        project_id,
        report_id,
        &input.definition_id,
        input.definition_version,
        extract_actor(&headers)?,
    )
    .await
    .map_err(map_review_preparation_error)?;
    accepted_review_run(snapshot)
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/records/{record_id}/ai/deduplication",
    operation_id = "generateDuplicateSuggestion",
    tag = "ai",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("record_id" = Uuid, Path, description = "Source record identifier")
    ),
    request_body = GenerateDuplicateRequest,
    responses(
        (status = 202, description = "Compiled duplicate review scheduled", body = ReviewRunDto),
        (status = 400, description = "Invalid AI request", body = ErrorResponse),
        (status = 404, description = "Record or candidate report not found", body = ErrorResponse),
        (status = 409, description = "A proposal conflicts", body = ErrorResponse),
        (status = 503, description = "AI provider is unavailable", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn generate_duplicate_suggestion(
    State(state): State<AppState>,
    Path((project_id, record_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(input): Json<GenerateDuplicateRequest>,
) -> Result<AcceptedReviewRun, ApiError> {
    let snapshot = deepref_postgres::schedule_duplicate_detection_review(
        &state.pool,
        project_id,
        record_id,
        input.candidate_report_id,
        extract_actor(&headers)?,
    )
    .await
    .map_err(map_review_preparation_error)?;
    accepted_review_run(snapshot)
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/review-runs/{run_id}",
    operation_id = "getReviewRun",
    tag = "ai",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("run_id" = Uuid, Path, description = "Review run identifier")
    ),
    responses(
        (status = 200, description = "Compiled review run status and result linkage", body = ReviewRunDto),
        (status = 404, description = "Review run not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_review_run(
    State(state): State<AppState>,
    Path((project_id, run_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ReviewRunDto>, ApiError> {
    let run_id = deepref_review::ReviewRunId::new(run_id)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let snapshot = deepref_postgres::get_review_run(&state.pool, project_id.into(), run_id)
        .await
        .map_err(map_postgres_review_error)?;
    Ok(Json(review_run_dto(snapshot)?))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/ai/proposals",
    operation_id = "listAiProposals",
    tag = "ai",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("cursor" = Option<String>, Query, description = "Opaque pagination cursor"),
        ("limit" = Option<i64>, Query, description = "Page size from 1 through 100"),
        ("status" = Option<String>, Query, description = "pending, accepted, rejected, or expired"),
        ("task_kind" = Option<String>, Query, description = "AI task kind filter"),
        ("target_report_id" = Option<Uuid>, Query, description = "Screening report target"),
        ("target_record_id" = Option<Uuid>, Query, description = "Dedupe source record target"),
        ("candidate_report_id" = Option<Uuid>, Query, description = "Dedupe candidate report target"),
        ("target_study_id" = Option<Uuid>, Query, description = "Study-scoped proposal target")
    ),
    responses(
        (status = 200, description = "Project-scoped AI proposals", body = PaginatedResponse<AiProposalDto>),
        (status = 400, description = "Invalid pagination or filter", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_ai_proposals(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Query(params): Query<AiProposalListParams>,
) -> Result<Json<PaginatedResponse<AiProposalDto>>, ApiError> {
    let pagination = PaginationParams {
        cursor: params.cursor,
        limit: params.limit,
    };
    let limit = pagination.limit()?;
    let cursor = pagination.decode::<(DateTime<Utc>, Uuid)>()?;
    let proposals = deepref_postgres::list_ai_proposals(
        &state.pool,
        project_id,
        deepref_postgres::AiProposalFilters {
            status: params.status.as_deref(),
            task_kind: params.task_kind.as_deref(),
            target_report_id: params.target_report_id,
            target_record_id: params.target_record_id,
            candidate_report_id: params.candidate_report_id,
            target_study_id: params.target_study_id,
        },
        cursor.map(|(created_at, id)| deepref_postgres::AiProposalCursor { created_at, id }),
        limit,
    )
    .await
    .map_err(map_ai_proposal_error)?;
    let items = proposals
        .into_iter()
        .map(proposal_dto)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(page(items, limit as usize, |item| {
        (item.created_at, item.id)
    })?))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/ai/proposals/{proposal_id}",
    operation_id = "getAiProposal",
    tag = "ai",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("proposal_id" = Uuid, Path, description = "AI proposal identifier")
    ),
    responses(
        (status = 200, description = "AI proposal", body = AiProposalDto),
        (status = 404, description = "AI proposal not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_ai_proposal(
    State(state): State<AppState>,
    Path((project_id, proposal_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<AiProposalDto>, ApiError> {
    Ok(Json(proposal_dto(
        deepref_postgres::get_ai_proposal(&state.pool, project_id, proposal_id)
            .await
            .map_err(map_ai_proposal_error)?,
    )?))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/ai/proposals/{proposal_id}/decision",
    operation_id = "decideAiProposal",
    tag = "ai",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("proposal_id" = Uuid, Path, description = "AI proposal identifier")
    ),
    request_body = DecideAiProposalRequest,
    responses(
        (status = 200, description = "AI proposal resolution", body = AiProposalDecisionDto),
        (status = 400, description = "Invalid decision", body = ErrorResponse),
        (status = 404, description = "AI proposal target not found", body = ErrorResponse),
        (status = 409, description = "Proposal or domain revision conflict", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn decide_ai_proposal(
    State(state): State<AppState>,
    Path((project_id, proposal_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(input): Json<DecideAiProposalRequest>,
) -> Result<Json<AiProposalDecisionDto>, ApiError> {
    if input.reason.trim().is_empty() || input.reason.len() > 2_000 {
        return Err(ApiError::BadRequest(
            "reason must be between 1 and 2000 characters".to_owned(),
        ));
    }
    let actor = extract_actor(&headers)?;
    let decision = match input.decision {
        AiProposalDecisionInput::Accept => AiProposalDecision::Accept,
        AiProposalDecisionInput::Reject => AiProposalDecision::Reject,
    };
    let resolution = deepref_postgres::decide_ai_proposal(
        &state.pool,
        AiProposalDecisionRequest {
            project_id,
            proposal_id,
            decision,
            reason: input.reason,
            reviewed_payload: input
                .reviewed_payload
                .map(reviewed_payload_to_internal)
                .transpose()?,
            actor,
        },
    )
    .await
    .map_err(map_ai_proposal_error)?;
    let proposal = deepref_postgres::get_ai_proposal(&state.pool, project_id, proposal_id)
        .await
        .map_err(map_ai_proposal_error)?;
    Ok(Json(AiProposalDecisionDto {
        proposal: proposal_dto(proposal)?,
        applied_revision: resolution.applied_revision,
    }))
}

fn reviewed_payload_to_internal(
    payload: AiReviewedProposalPayload,
) -> Result<ReviewedAiProposalPayload, ApiError> {
    match payload {
        AiReviewedProposalPayload::AppraisalPrefill {
            report_id,
            definition_id,
            definition_version,
            answers,
            domain_judgments,
            overall_judgment,
        } => Ok(ReviewedAiProposalPayload::AppraisalPrefill(
            deepref_ai::AppraisalPrefill {
                report_id,
                definition_id,
                definition_version,
                answers: serde_json::from_value(serde_json::to_value(answers).map_err(
                    |error| ApiError::BadRequest(format!("reviewed payload is invalid: {error}")),
                )?)
                .map_err(|error| {
                    ApiError::BadRequest(format!("reviewed payload is invalid: {error}"))
                })?,
                domain_judgments,
                overall_judgment,
            },
        )),
        AiReviewedProposalPayload::DataExtraction { study_id, fields } => Ok(
            ReviewedAiProposalPayload::DataExtraction(deepref_ai::DataExtraction {
                study_id,
                fields: serde_json::from_value(serde_json::to_value(fields).map_err(|error| {
                    ApiError::BadRequest(format!("reviewed payload is invalid: {error}"))
                })?)
                .map_err(|error| {
                    ApiError::BadRequest(format!("reviewed payload is invalid: {error}"))
                })?,
            }),
        ),
    }
}

pub(super) fn accepted_review_run(
    snapshot: deepref_review::ReviewRunSnapshot,
) -> Result<AcceptedReviewRun, ApiError> {
    let location = format!(
        "/projects/{}/review-runs/{}",
        snapshot.project_id.as_uuid(),
        snapshot.id.as_uuid()
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        LOCATION,
        location.parse().map_err(|error| {
            ApiError::Internal(anyhow::anyhow!("invalid run location: {error}"))
        })?,
    );
    Ok((
        StatusCode::ACCEPTED,
        headers,
        Json(review_run_dto(snapshot)?),
    ))
}

fn review_run_dto(snapshot: deepref_review::ReviewRunSnapshot) -> Result<ReviewRunDto, ApiError> {
    let state = match snapshot.state {
        deepref_review::ReviewRunState::Queued => ReviewRunStateDto::Queued,
        deepref_review::ReviewRunState::Running => ReviewRunStateDto::Running,
        deepref_review::ReviewRunState::Blocked { code, message } => ReviewRunStateDto::Blocked {
            code: code.as_str().to_owned(),
            message,
        },
        deepref_review::ReviewRunState::Failed { code, message } => {
            ReviewRunStateDto::Failed { code, message }
        }
        deepref_review::ReviewRunState::Completed { proposal_id } => {
            ReviewRunStateDto::Completed { proposal_id }
        }
    };
    Ok(ReviewRunDto {
        id: snapshot.id.as_uuid(),
        project_id: snapshot.project_id.as_uuid(),
        definition: snapshot.definition.as_str().to_owned(),
        subject: serde_json::to_value(snapshot.subject)
            .map_err(|error| ApiError::Internal(error.into()))?,
        origin: serde_json::to_value(snapshot.origin)
            .map_err(|error| ApiError::Internal(error.into()))?,
        state,
        created_at: snapshot.created_at,
        started_at: snapshot.started_at,
        finished_at: snapshot.finished_at,
    })
}

pub(crate) fn proposal_dto(proposal: AiProposalRecord) -> Result<AiProposalDto, ApiError> {
    let payload = typed_payload(&proposal)?;
    Ok(AiProposalDto {
        id: proposal.id,
        project_id: proposal.project_id,
        task_kind: proposal.task_kind,
        entity_type: proposal.entity_type,
        entity_id: proposal.entity_id,
        operation: proposal.operation,
        payload,
        authority_tier: proposal.authority_tier,
        model_run_id: proposal.model_run_id,
        provider: proposal.provider,
        model: proposal.model,
        model_version: proposal.model_version,
        prompt_version: proposal.prompt_version,
        schema_version: proposal.schema_version,
        prompt_hash: proposal.prompt_hash,
        schema_hash: proposal.schema_hash,
        input_hash: proposal.input_hash,
        evidence_hash: proposal.evidence_hash,
        status: proposal.status,
        protocol_version_id: proposal.protocol_version_id,
        expected_revision: proposal.expected_revision,
        target_report_id: proposal.target_report_id,
        target_record_id: proposal.target_record_id,
        target_study_id: proposal.target_study_id,
        resolved_at: proposal.resolved_at,
        resolved_by_actor_kind: proposal.resolved_by_actor_kind,
        resolved_by_actor_id: proposal.resolved_by_actor_id,
        resolution_reason: proposal.resolution_reason,
        created_at: proposal.created_at,
    })
}

fn typed_payload(proposal: &AiProposalRecord) -> Result<AiProposalPayload, ApiError> {
    let mut payload = proposal.payload.clone();
    let object = payload.as_object_mut().ok_or_else(|| {
        ApiError::Internal(anyhow::anyhow!(
            "stored AI proposal payload is not an object"
        ))
    })?;
    let kind = match proposal.task_kind.as_str() {
        "title_abstract_screening" | "full_text_screening" => "screening",
        "duplicate_candidate_detection" => "duplicate",
        "study_grouping" => "study_grouping",
        "appraisal_prefill" => "appraisal_prefill",
        "data_extraction" => "data_extraction",
        task_kind => {
            return Err(ApiError::Internal(anyhow::anyhow!(
                "unsupported AI proposal task kind: {task_kind}"
            )));
        }
    };
    object.insert("kind".to_owned(), Value::String(kind.to_owned()));
    serde_json::from_value(payload).map_err(|error| {
        ApiError::Internal(anyhow::anyhow!(
            "stored AI proposal payload is invalid: {error}"
        ))
    })
}

fn map_ai_error(error: AiError) -> ApiError {
    match error {
        AiError::InvalidContext(message)
        | AiError::SemanticValidation(message)
        | AiError::SchemaValidation(message)
        | AiError::MalformedOutput(message)
        | AiError::InputSerialization(message) => ApiError::BadRequest(message),
        AiError::Route(message) => ApiError::Configuration(message),
        AiError::Gateway(message) => {
            ApiError::Configuration(format!("AI provider is unavailable: {message}"))
        }
        AiError::Persistence(message) | AiError::Proposal(message) => {
            ApiError::Internal(anyhow::anyhow!(message))
        }
        AiError::PromptRegistry(message) | AiError::InvalidEmbedding(message) => {
            ApiError::BadRequest(message)
        }
    }
}

pub(super) fn map_review_preparation_error(
    error: deepref_postgres::ReviewPreparationError,
) -> ApiError {
    match error {
        deepref_postgres::ReviewPreparationError::Review(error) => map_postgres_review_error(error),
        deepref_postgres::ReviewPreparationError::Protocol(error) => map_protocol_error(error),
        deepref_postgres::ReviewPreparationError::AiProposal(error) => map_ai_proposal_error(error),
        deepref_postgres::ReviewPreparationError::Extraction(error) => match error {
            deepref_postgres::ExtractionError::Database(error) => ApiError::Database(error),
            deepref_postgres::ExtractionError::DefinitionNotFound
            | deepref_postgres::ExtractionError::StudyNotFound => {
                ApiError::NotFound(error.to_string())
            }
            _ => ApiError::BadRequest(error.to_string()),
        },
        deepref_postgres::ReviewPreparationError::InvalidInput(message)
            if message.contains("changed") =>
        {
            ApiError::Conflict {
                code: "review_subject_changed".to_owned(),
                message,
                details: Value::Null,
            }
        }
        deepref_postgres::ReviewPreparationError::InvalidInput(message) => {
            ApiError::BadRequest(message)
        }
    }
}

fn map_postgres_review_error(error: deepref_postgres::PostgresReviewError) -> ApiError {
    match error {
        deepref_postgres::PostgresReviewError::Database(error) => ApiError::Database(error),
        deepref_postgres::PostgresReviewError::Serialization(error) => {
            ApiError::Internal(error.into())
        }
        deepref_postgres::PostgresReviewError::Review(error) => {
            ApiError::Configuration(error.to_string())
        }
        deepref_postgres::PostgresReviewError::Ai(error) => map_ai_error(error),
        deepref_postgres::PostgresReviewError::RunNotFound => {
            ApiError::NotFound("review run not found".to_owned())
        }
        deepref_postgres::PostgresReviewError::InvalidState(message) => ApiError::Conflict {
            code: "review_run_state_conflict".to_owned(),
            message,
            details: Value::Null,
        },
        deepref_postgres::PostgresReviewError::InvalidStoredValue(message) => {
            ApiError::Internal(anyhow::anyhow!(message))
        }
        deepref_postgres::PostgresReviewError::WorkerOwnership => {
            ApiError::Internal(anyhow::anyhow!("review worker lease is not owned"))
        }
        deepref_postgres::PostgresReviewError::FinalizationConflict => ApiError::Conflict {
            code: "review_finalization_conflict".to_owned(),
            message: "review proposal finalization conflicts with persisted state".to_owned(),
            details: Value::Null,
        },
    }
}

fn map_ai_proposal_error(error: AiProposalError) -> ApiError {
    match error {
        AiProposalError::Database(error) => ApiError::Database(error),
        AiProposalError::NotFound => ApiError::NotFound("AI proposal target not found".to_owned()),
        AiProposalError::NotPending => ApiError::Conflict {
            code: "ai_proposal_not_pending".to_owned(),
            message: "AI proposal is no longer pending".to_owned(),
            details: Value::Null,
        },
        AiProposalError::InvalidPayload(message) | AiProposalError::InvalidTarget(message) => {
            ApiError::BadRequest(message)
        }
        AiProposalError::InvalidActor => ApiError::BadRequest("actor is invalid".to_owned()),
        AiProposalError::Screening(error) => super::review::map_screening_error(error),
        AiProposalError::Dedupe(error) => super::deduplication::map_dedupe_error(error),
        AiProposalError::Study(error) => super::study::map_study_error(error),
        AiProposalError::Appraisal(error) => super::study::map_appraisal_error(error),
        AiProposalError::Extraction(
            error @ (deepref_postgres::ExtractionError::EvidenceNotInStudy
            | deepref_postgres::ExtractionError::RequiredFieldInsufficient
            | deepref_postgres::ExtractionError::StaleDefinitionVersion
            | deepref_postgres::ExtractionError::ValueAlreadyApproved),
        ) => ApiError::Conflict {
            code: "extraction_conflict".to_owned(),
            message: error.to_string(),
            details: Value::Null,
        },
        AiProposalError::Extraction(
            error @ (deepref_postgres::ExtractionError::DefinitionNotFound
            | deepref_postgres::ExtractionError::StudyNotFound),
        ) => ApiError::NotFound(error.to_string()),
        AiProposalError::Extraction(error) => ApiError::BadRequest(error.to_string()),
    }
}

fn map_protocol_error(error: deepref_postgres::ProtocolError) -> ApiError {
    match error {
        deepref_postgres::ProtocolError::ProjectNotFound
        | deepref_postgres::ProtocolError::NotFound => ApiError::NotFound(error.to_string()),
        deepref_postgres::ProtocolError::Database(error) => ApiError::Database(error),
        deepref_postgres::ProtocolError::Serialization(error) => ApiError::Internal(error.into()),
        deepref_postgres::ProtocolError::Invalid(message)
        | deepref_postgres::ProtocolError::DataIntegrity(message) => ApiError::BadRequest(message),
        deepref_postgres::ProtocolError::DraftAlreadyExists
        | deepref_postgres::ProtocolError::NotEditable => ApiError::Conflict {
            code: "protocol_conflict".to_owned(),
            message: error.to_string(),
            details: Value::Null,
        },
        deepref_postgres::ProtocolError::Conflict { message, .. } => ApiError::Conflict {
            code: "protocol_conflict".to_owned(),
            message: message.to_owned(),
            details: Value::Null,
        },
    }
}
