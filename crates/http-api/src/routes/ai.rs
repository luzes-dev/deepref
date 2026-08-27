use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use chrono::{DateTime, Utc};
use deepref_ai::{
    AiError, AiTaskRunner, CriterionPrompt, DedupeInput, DedupeTask, IdentityProvenance,
    ScreeningEvidence, ScreeningEvidenceField, ScreeningInput, ScreeningStage, ScreeningTask,
    ScreeningTaskConfig, SystemClock, UuidProvider,
};
use deepref_application::{DedupeCandidate, FUZZY_PROPOSAL_THRESHOLD, score_candidate};
use deepref_domain::{
    CriterionStage, EligibilityCriterion, ScreeningStage as DomainScreeningStage,
};
use deepref_postgres::{
    AiProposalDecision, AiProposalDecisionRequest, AiProposalError, AiProposalRecord,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
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
    fn domain(self) -> DomainScreeningStage {
        match self {
            Self::TitleAbstract => DomainScreeningStage::TitleAbstract,
            Self::FullText => DomainScreeningStage::FullText,
        }
    }

    fn ai(self) -> ScreeningStage {
        match self {
            Self::TitleAbstract => ScreeningStage::TitleAbstract,
            Self::FullText => ScreeningStage::FullText,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
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
#[serde(rename_all = "snake_case")]
pub(crate) enum AiProposalDecisionInput {
    Accept,
    Reject,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct DecideAiProposalRequest {
    pub decision: AiProposalDecisionInput,
    pub reason: String,
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
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AiProposalPayload {
    Screening(AiScreeningProposalPayload),
    Duplicate(AiDuplicateProposalPayload),
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
    pub status: String,
    pub protocol_version_id: Option<Uuid>,
    pub expected_revision: Option<i64>,
    pub target_report_id: Option<Uuid>,
    pub target_record_id: Option<Uuid>,
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
        (status = 200, description = "AI screening proposal", body = AiProposalDto),
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
    Json(input): Json<GenerateScreeningRequest>,
) -> Result<Json<AiProposalDto>, ApiError> {
    let stage = input.stage.ai();
    let protocol = deepref_postgres::get_published_protocol(&state.pool, project_id)
        .await
        .map_err(map_protocol_error)?;
    if input
        .protocol_version_id
        .is_some_and(|id| id != protocol.id)
    {
        return Err(ApiError::Conflict {
            code: "ai_protocol_changed".to_owned(),
            message: "the requested protocol is not the current published version".to_owned(),
            details: json!({"protocolVersionId": protocol.id}),
        });
    }
    let target = deepref_postgres::get_ai_screening_target(&state.pool, project_id, report_id)
        .await
        .map_err(map_ai_proposal_error)?;
    let expected_revision = input.expected_revision.unwrap_or(target.expected_revision);
    let allowed_reasons =
        deepref_postgres::list_ai_exclusion_reasons(&state.pool, project_id, input.stage.domain())
            .await
            .map_err(map_ai_proposal_error)?;
    let allowed_evidence = metadata_evidence(report_id, &target);
    let criteria = protocol.criteria.clone();
    let task = ScreeningTask::new(ScreeningTaskConfig {
        project_id: project_id.into(),
        report_id: report_id.into(),
        stage,
        protocol_version_id: protocol.id.into(),
        expected_revision,
        criteria: criteria.clone(),
        allowed_evidence,
        allowed_exclusion_reasons: allowed_reasons.into_iter().collect(),
    });
    let prompts = criteria.iter().map(criterion_prompt).collect::<Vec<_>>();
    let ai_input = ScreeningInput {
        project_id: project_id.into(),
        report_id: report_id.into(),
        stage,
        protocol_version_id: protocol.id.into(),
        expected_revision,
        title: target.title.clone(),
        abstract_text: target.abstract_text.clone(),
        document_hash: None,
        retrieval_query: (stage == ScreeningStage::FullText)
            .then(|| screening_retrieval_query(&target, &criteria)),
        criteria: prompts,
    };
    let proposal = run_task(&state, &task, ai_input).await?;
    Ok(Json(proposal_dto(proposal)?))
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
        (status = 200, description = "AI duplicate assistance proposal", body = AiProposalDto),
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
    Json(input): Json<GenerateDuplicateRequest>,
) -> Result<Json<AiProposalDto>, ApiError> {
    let target = deepref_postgres::get_ai_dedupe_target(
        &state.pool,
        project_id,
        record_id,
        input.candidate_report_id,
    )
    .await
    .map_err(map_ai_proposal_error)?;
    let source_id = record_id;
    let candidate_id = input.candidate_report_id;
    let provenance = dedupe_provenance(source_id, candidate_id, &target);
    let signals = dedupe_signals(candidate_id, &target);
    let task = DedupeTask::new(
        project_id.into(),
        record_id.into(),
        candidate_id.into(),
        provenance.clone(),
        signals.clone(),
    );
    let ai_input = DedupeInput {
        project_id: project_id.into(),
        source_record_id: record_id.into(),
        candidate_report_id: candidate_id.into(),
        source_title: target.source_title.clone(),
        candidate_title: target.candidate_title.clone(),
        source_year: target.source_year,
        candidate_year: target.candidate_year,
        source_author: target.source_author.clone(),
        candidate_author: target.candidate_author.clone(),
        source_title_hash: target.source_title_hash.clone(),
        candidate_title_hash: target.candidate_title_hash.clone(),
        grounded_signals: signals,
        grounded_provenance: provenance,
    };
    let proposal = run_task(&state, &task, ai_input).await?;
    Ok(Json(proposal_dto(proposal)?))
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
        ("candidate_report_id" = Option<Uuid>, Query, description = "Dedupe candidate report target")
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

async fn run_task<T>(
    state: &AppState,
    task: &T,
    input: T::Input,
) -> Result<AiProposalRecord, ApiError>
where
    T: deepref_ai::AiTask,
{
    let store = deepref_postgres::PostgresAiStore::new(&state.pool);
    let runner = AiTaskRunner::new(
        state.ai_gateway.as_ref(),
        &store,
        &store,
        &store,
        &store,
        &SystemClock,
        &UuidProvider,
    );
    let result = runner.run(task, input).await.map_err(map_ai_error)?;
    let proposal = result
        .proposal
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("AI task did not produce a proposal")))?;
    deepref_postgres::get_ai_proposal(
        &state.pool,
        proposal.draft.project_id.as_uuid(),
        proposal.id,
    )
    .await
    .map_err(map_ai_proposal_error)
}

fn metadata_evidence(
    report_id: Uuid,
    target: &deepref_postgres::AiScreeningTarget,
) -> Vec<ScreeningEvidence> {
    let mut evidence = Vec::new();
    if let Some(title) = &target.title {
        evidence.push(ScreeningEvidence::ReportMetadata {
            report_id,
            field: ScreeningEvidenceField::Title,
            content_hash: deepref_ai::sha256_bytes(title.as_bytes()),
        });
    }
    if let Some(abstract_text) = &target.abstract_text {
        evidence.push(ScreeningEvidence::ReportMetadata {
            report_id,
            field: ScreeningEvidenceField::Abstract,
            content_hash: deepref_ai::sha256_bytes(abstract_text.as_bytes()),
        });
    }
    evidence
}

fn criterion_prompt(criterion: &EligibilityCriterion) -> CriterionPrompt {
    CriterionPrompt {
        id: criterion.id,
        label: criterion.label.clone(),
        description: criterion.description.clone(),
        ordinal: criterion.ordinal,
        kind: match criterion.kind {
            deepref_domain::CriterionKind::Inclusion => "inclusion".to_owned(),
            deepref_domain::CriterionKind::Exclusion => "exclusion".to_owned(),
        },
        stage: match criterion.stage {
            CriterionStage::TitleAbstract => "title_abstract",
            CriterionStage::FullText => "full_text",
            CriterionStage::Both => "both",
        }
        .to_owned(),
    }
}

fn screening_retrieval_query(
    target: &deepref_postgres::AiScreeningTarget,
    criteria: &[EligibilityCriterion],
) -> String {
    const MAX_TERMS: usize = 64;
    const MAX_TERM_CHARS: usize = 48;
    let mut terms = Vec::new();
    let mut seen = BTreeSet::new();
    let mut add_terms = |text: &str| {
        let mut token = String::new();
        let mut flush = |token: &mut String| {
            if token.is_empty() {
                return;
            }
            let normalized: String = token
                .chars()
                .flat_map(char::to_lowercase)
                .take(MAX_TERM_CHARS)
                .collect();
            let char_count = normalized.chars().count();
            if (char_count >= 3
                || normalized
                    .chars()
                    .all(|character| character.is_ascii_digit()))
                && seen.insert(normalized.clone())
                && terms.len() < MAX_TERMS
            {
                terms.push(normalized);
            }
            token.clear();
        };
        for character in text.chars() {
            if character.is_alphanumeric() {
                token.push(character);
            } else {
                flush(&mut token);
            }
        }
        flush(&mut token);
    };
    for criterion in criteria {
        add_terms(&criterion.label);
        add_terms(&criterion.description);
    }
    if let Some(title) = &target.title {
        add_terms(title);
    }
    if let Some(abstract_text) = &target.abstract_text {
        add_terms(abstract_text);
    }
    if terms.is_empty() {
        "full-text eligibility evidence".to_owned()
    } else {
        terms.into_iter().collect::<Vec<_>>().join(" OR ")
    }
}

fn dedupe_provenance(
    source_record_id: Uuid,
    candidate_report_id: Uuid,
    target: &deepref_postgres::AiDedupeTarget,
) -> Vec<IdentityProvenance> {
    let mut provenance = Vec::new();
    let mut push = |entity_type: &str, entity_id: Uuid, field: &str, value: &str| {
        provenance.push(IdentityProvenance {
            entity_type: entity_type.to_owned(),
            entity_id: entity_id.to_string(),
            field: field.to_owned(),
            content_hash: deepref_ai::sha256_bytes(value.as_bytes()),
        });
    };
    if let Some(title) = target.source_title.as_deref() {
        push("record", source_record_id, "title", title);
    }
    if let Some(title) = target.candidate_title.as_deref() {
        push("report", candidate_report_id, "title", title);
    }
    if let Some(year) = target.source_year {
        push(
            "record",
            source_record_id,
            "publication_year",
            &year.to_string(),
        );
    }
    if let Some(year) = target.candidate_year {
        push(
            "report",
            candidate_report_id,
            "publication_year",
            &year.to_string(),
        );
    }
    if let Some(author) = target.source_author.as_deref() {
        push("record", source_record_id, "first_author", author);
    }
    if let Some(author) = target.candidate_author.as_deref() {
        push("report", candidate_report_id, "first_author", author);
    }
    provenance
}

fn dedupe_signals(
    candidate_report_id: Uuid,
    target: &deepref_postgres::AiDedupeTarget,
) -> Vec<deepref_ai::DuplicateSignal> {
    let candidate = DedupeCandidate {
        report_id: candidate_report_id.into(),
        title: target.candidate_title.clone(),
        first_author: target.candidate_author.clone(),
        publication_year: target.candidate_year,
        exact_identifier_match: false,
        conflicting_identifier: false,
    };
    let score = score_candidate(
        target.source_title.as_deref(),
        target.source_author.as_deref(),
        target.source_year,
        &candidate,
    );
    let mut signals = Vec::new();
    if target.source_title.is_some() && target.candidate_title.is_some() {
        signals.push(deepref_ai::DuplicateSignal::TitleSimilarity {
            similarity: score.title_similarity,
            supports_match: score.title_similarity >= FUZZY_PROPOSAL_THRESHOLD,
        });
    }
    if let Some((source_year, candidate_year)) = target.source_year.zip(target.candidate_year) {
        signals.push(deepref_ai::DuplicateSignal::PublicationYear {
            source_year,
            candidate_year,
            supports_match: score.year_match == Some(true),
        });
    }
    if let Some((source_author, candidate_author)) = target
        .source_author
        .as_ref()
        .zip(target.candidate_author.as_ref())
    {
        signals.push(deepref_ai::DuplicateSignal::FirstAuthor {
            source_author: source_author.clone(),
            candidate_author: candidate_author.clone(),
            similarity: score.first_author_similarity.unwrap_or_default(),
            supports_match: score
                .first_author_similarity
                .is_some_and(|similarity| similarity >= FUZZY_PROPOSAL_THRESHOLD),
        });
    }
    signals
}

fn proposal_dto(proposal: AiProposalRecord) -> Result<AiProposalDto, ApiError> {
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
        status: proposal.status,
        protocol_version_id: proposal.protocol_version_id,
        expected_revision: proposal.expected_revision,
        target_report_id: proposal.target_report_id,
        target_record_id: proposal.target_record_id,
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
