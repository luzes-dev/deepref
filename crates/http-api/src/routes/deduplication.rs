use axum::{
    Json,
    extract::{Path, Query, State},
};
use chrono::{DateTime, Utc};
use deepref_application::{ProposalDecision, RecordResolutionAction, ResolveRecordCommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

use super::pagination::{PaginatedResponse, PaginationParams, page};
use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};

const MAX_BATCH: i64 = 100;

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct RunDeduplicationRequest {
    /// Number of unresolved source records to process, from 1 through 100.
    pub limit: Option<i64>,
    pub actor_kind: Option<String>,
    pub actor_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DedupeRunDto {
    pub project_id: Uuid,
    pub processed: i64,
    pub auto_linked: i64,
    pub created_reports: i64,
    pub proposals_created: i64,
    pub conflicts: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ProposalListParams {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub status: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DedupeProposalDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub record_id: Uuid,
    pub candidate_report_id: Option<Uuid>,
    pub proposal_kind: String,
    pub source_title: Option<String>,
    pub source_abstract: Option<String>,
    pub source_year: Option<i32>,
    #[schema(value_type = Object)]
    pub source_authors: Value,
    #[schema(value_type = Object)]
    pub source_identifiers: Value,
    pub candidate_title: Option<String>,
    pub candidate_year: Option<i32>,
    #[schema(value_type = Object)]
    pub candidate_authors: Value,
    #[schema(value_type = Object)]
    pub candidate_identifiers: Value,
    pub title_similarity: f64,
    pub year_match: Option<bool>,
    pub first_author_similarity: Option<f64>,
    pub exact_identifier_match: bool,
    pub conflicting_identifier: bool,
    pub score: f64,
    #[schema(value_type = Object)]
    pub metadata: Value,
    pub status: String,
    pub revision: i64,
    pub reviewer_kind: Option<String>,
    pub reviewer_id: Option<String>,
    pub decided_at: Option<DateTime<Utc>>,
    pub decision_reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ProposalDecisionRequest {
    pub decision: ProposalDecisionInput,
    pub reason: String,
    pub actor_kind: Option<String>,
    pub actor_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProposalDecisionInput {
    Accept,
    Reject,
    CreateNew,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ResolutionDto {
    pub record_id: Uuid,
    pub prior_report_id: Option<Uuid>,
    pub resolved_report_id: Option<Uuid>,
    pub action: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct RecordResolutionRequest {
    pub action: RecordResolutionActionInput,
    pub report_id: Option<Uuid>,
    pub proposal_id: Option<Uuid>,
    pub reason: String,
    pub actor_kind: Option<String>,
    pub actor_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RecordResolutionActionInput {
    Create,
    Link,
    Reassign,
    Revert,
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/deduplication/run",
    operation_id = "runProjectDeduplication",
    tag = "deduplication",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    request_body = RunDeduplicationRequest,
    responses(
        (status = 200, description = "Bounded deduplication run summary", body = DedupeRunDto),
        (status = 400, description = "Invalid run bounds or actor", body = ErrorResponse),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn run_project_deduplication(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Json(input): Json<RunDeduplicationRequest>,
) -> Result<Json<DedupeRunDto>, ApiError> {
    let limit = bounded_limit(input.limit)?;
    let (actor_kind, actor_id) = actor(input.actor_kind, input.actor_id)?;
    let summary = deepref_postgres::run_deduplication(
        &state.pool,
        deepref_postgres::DedupeRunRequest {
            project_id,
            limit,
            actor_kind,
            actor_id,
        },
    )
    .await
    .map_err(map_dedupe_error)?;
    Ok(Json(DedupeRunDto {
        project_id,
        processed: summary.processed,
        auto_linked: summary.auto_linked,
        created_reports: summary.created_reports,
        proposals_created: summary.proposals_created,
        conflicts: summary.conflicts,
    }))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/deduplication/proposals",
    operation_id = "listProjectDedupeProposals",
    tag = "deduplication",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("cursor" = Option<String>, Query, description = "Opaque pagination cursor"),
        ("limit" = Option<i64>, Query, description = "Page size from 1 through 100"),
        ("status" = Option<String>, Query, description = "pending, accepted, or rejected")
    ),
    responses(
        (status = 200, description = "Dedupe proposals", body = PaginatedResponse<DedupeProposalDto>),
        (status = 400, description = "Invalid pagination or status", body = ErrorResponse),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_project_dedupe_proposals(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Query(params): Query<ProposalListParams>,
) -> Result<Json<PaginatedResponse<DedupeProposalDto>>, ApiError> {
    let limit = bounded_limit(params.limit)?;
    let status = params.status.unwrap_or_else(|| "pending".to_owned());
    if !matches!(status.as_str(), "pending" | "accepted" | "rejected") {
        return Err(ApiError::BadRequest(
            "status must be pending, accepted, or rejected".to_owned(),
        ));
    }
    let pagination = PaginationParams {
        cursor: params.cursor,
        limit: Some(limit),
    };
    let cursor = pagination.decode::<(DateTime<Utc>, Uuid)>()?;
    let proposals = deepref_postgres::list_proposals(
        &state.pool,
        project_id,
        &status,
        cursor.map(|(created_at, id)| deepref_postgres::DedupeProposalCursor { created_at, id }),
        limit,
    )
    .await
    .map_err(map_dedupe_error)?;
    let items = proposals.into_iter().map(proposal_dto).collect::<Vec<_>>();
    Ok(Json(page(items, limit as usize, |item| {
        (item.created_at, item.id)
    })?))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/deduplication/proposals/{proposal_id}/decision",
    operation_id = "decideProjectDedupeProposal",
    tag = "deduplication",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("proposal_id" = Uuid, Path, description = "Proposal identifier")
    ),
    request_body = ProposalDecisionRequest,
    responses(
        (status = 200, description = "Resolution result", body = ResolutionDto),
        (status = 400, description = "Invalid decision", body = ErrorResponse),
        (status = 404, description = "Proposal or report not found", body = ErrorResponse),
        (status = 409, description = "Proposal is no longer pending or create-new is invalid for an identifier conflict", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn decide_project_dedupe_proposal(
    State(state): State<AppState>,
    Path((project_id, proposal_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<ProposalDecisionRequest>,
) -> Result<Json<ResolutionDto>, ApiError> {
    let (actor_kind, actor_id) = actor(input.actor_kind, input.actor_id)?;
    let result = deepref_postgres::decide_proposal(
        &state.pool,
        deepref_postgres::ProposalDecisionRequest {
            project_id,
            proposal_id,
            decision: input.decision.domain(),
            reason: input.reason,
            actor_kind,
            actor_id,
        },
    )
    .await
    .map_err(map_dedupe_error)?;
    Ok(Json(resolution_dto(result)))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/records/{record_id}/resolution",
    operation_id = "resolveProjectRecord",
    tag = "deduplication",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("record_id" = Uuid, Path, description = "Source record identifier")
    ),
    request_body = RecordResolutionRequest,
    responses(
        (status = 200, description = "Resolution result", body = ResolutionDto),
        (status = 400, description = "Invalid resolution", body = ErrorResponse),
        (status = 404, description = "Record or report not found", body = ErrorResponse),
        (status = 409, description = "Resolution conflict, including an exhausted revert history", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn resolve_project_record(
    State(state): State<AppState>,
    Path((project_id, record_id)): Path<(Uuid, Uuid)>,
    Json(input): Json<RecordResolutionRequest>,
) -> Result<Json<ResolutionDto>, ApiError> {
    let (actor_kind, actor_id) = actor(input.actor_kind, input.actor_id)?;
    let result = deepref_postgres::resolve_record(
        &state.pool,
        ResolveRecordCommand {
            project_id: project_id.into(),
            record_id: record_id.into(),
            action: input.action.domain(),
            report_id: input.report_id.map(Into::into),
            proposal_id: input.proposal_id,
            reason: input.reason,
            actor_kind,
            actor_id,
        },
    )
    .await
    .map_err(map_dedupe_error)?;
    Ok(Json(resolution_dto(result)))
}

impl ProposalDecisionInput {
    fn domain(self) -> ProposalDecision {
        match self {
            Self::Accept => ProposalDecision::Accept,
            Self::Reject => ProposalDecision::Reject,
            Self::CreateNew => ProposalDecision::CreateNew,
        }
    }
}

impl RecordResolutionActionInput {
    fn domain(self) -> RecordResolutionAction {
        match self {
            Self::Create => RecordResolutionAction::Create,
            Self::Link => RecordResolutionAction::Link,
            Self::Reassign => RecordResolutionAction::Reassign,
            Self::Revert => RecordResolutionAction::Revert,
        }
    }
}

fn bounded_limit(limit: Option<i64>) -> Result<i64, ApiError> {
    let limit = limit.unwrap_or(50);
    if !(1..=MAX_BATCH).contains(&limit) {
        return Err(ApiError::BadRequest(
            "limit must be between 1 and 100".to_owned(),
        ));
    }
    Ok(limit)
}

fn actor(kind: Option<String>, id: Option<String>) -> Result<(String, String), ApiError> {
    let kind = kind.unwrap_or_else(|| "user".to_owned());
    let id = id.unwrap_or_else(|| "web-user".to_owned());
    if !matches!(kind.as_str(), "user" | "automation" | "system") || id.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "actor_kind must be user, automation, or system and actor_id must not be empty"
                .to_owned(),
        ));
    }
    Ok((kind, id))
}

fn proposal_dto(proposal: deepref_postgres::DedupeProposal) -> DedupeProposalDto {
    DedupeProposalDto {
        id: proposal.id,
        project_id: proposal.project_id,
        record_id: proposal.record_id,
        candidate_report_id: proposal.candidate_report_id,
        proposal_kind: proposal.proposal_kind,
        source_title: proposal.source_title,
        source_abstract: proposal.source_abstract,
        source_year: proposal.source_year,
        source_authors: proposal.source_authors,
        source_identifiers: proposal.source_identifiers,
        candidate_title: proposal.candidate_title,
        candidate_year: proposal.candidate_year,
        candidate_authors: proposal.candidate_authors,
        candidate_identifiers: proposal.candidate_identifiers,
        title_similarity: proposal.title_similarity,
        year_match: proposal.year_match,
        first_author_similarity: proposal.first_author_similarity,
        exact_identifier_match: proposal.exact_identifier_match,
        conflicting_identifier: proposal.conflicting_identifier,
        score: proposal.score,
        metadata: proposal.metadata,
        status: proposal.status,
        revision: proposal.revision,
        reviewer_kind: proposal.reviewer_kind,
        reviewer_id: proposal.reviewer_id,
        decided_at: proposal.decided_at,
        decision_reason: proposal.decision_reason,
        created_at: proposal.created_at,
    }
}

fn resolution_dto(result: deepref_postgres::ResolutionResult) -> ResolutionDto {
    ResolutionDto {
        record_id: result.record_id,
        prior_report_id: result.prior_report_id,
        resolved_report_id: result.resolved_report_id,
        action: result.action.as_str().to_owned(),
    }
}

fn map_dedupe_error(error: deepref_postgres::DedupeError) -> ApiError {
    match error {
        deepref_postgres::DedupeError::ProjectNotFound
        | deepref_postgres::DedupeError::RecordNotFound
        | deepref_postgres::DedupeError::ProposalNotFound
        | deepref_postgres::DedupeError::ReportNotInProject => {
            ApiError::NotFound(error.to_string())
        }
        deepref_postgres::DedupeError::ProposalNotPending => ApiError::Conflict {
            code: "DEDUPE_PROPOSAL_NOT_PENDING".to_owned(),
            message: error.to_string(),
            details: Value::Null,
        },
        deepref_postgres::DedupeError::ConflictCreateNew => ApiError::Conflict {
            code: "DEDUPE_CONFLICT_CREATE_NEW_NOT_ALLOWED".to_owned(),
            message: error.to_string(),
            details: Value::Null,
        },
        deepref_postgres::DedupeError::RevertConflict => ApiError::Conflict {
            code: "DEDUPE_REVERT_CONFLICT".to_owned(),
            message: error.to_string(),
            details: Value::Null,
        },
        deepref_postgres::DedupeError::IdentifierConflict => ApiError::Conflict {
            code: "DEDUPE_IDENTIFIER_CONFLICT".to_owned(),
            message: error.to_string(),
            details: Value::Null,
        },
        deepref_postgres::DedupeError::InvalidCommand(message) => ApiError::BadRequest(message),
        deepref_postgres::DedupeError::Database(error) => ApiError::Database(error),
        deepref_postgres::DedupeError::Serialization(error) => ApiError::Internal(error.into()),
    }
}
