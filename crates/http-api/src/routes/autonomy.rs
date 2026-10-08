//! AI autonomy settings, the activity feed with undo, and the AI second
//! reviewer's conflicts.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use chrono::{DateTime, Utc};
use deepref_application::workflows::{AutonomyLevel, AutonomyTask, LOCKED_TASKS};
use deepref_domain::ScreeningDecision;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::{
    actor::extract_actor,
    extraction::{ExtractionValueDto, map_extraction_error, value_dto},
    pagination::{PaginatedResponse, PaginationParams, page},
};
use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};

// ---------------------------------------------------------------------------
// Autonomy settings
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AiAutonomyTaskDto {
    /// Stable task key, for example `extraction`.
    pub task: String,
    pub label: String,
    pub description: String,
    /// `off`, `suggest`, `second_reviewer` or `act`.
    pub level: String,
    pub default_level: String,
    /// Levels that can be chosen for this task.
    pub allowed_levels: Vec<String>,
    pub is_default: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AiLockedTaskDto {
    pub task: String,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AiAutonomyDto {
    pub tasks: Vec<AiAutonomyTaskDto>,
    /// Work that is always a person's decision and cannot be automated.
    pub locked: Vec<AiLockedTaskDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct AiAutonomyChange {
    pub task: String,
    pub level: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct UpdateAiAutonomyRequest {
    pub changes: Vec<AiAutonomyChange>,
}

fn task_text(task: AutonomyTask) -> (&'static str, &'static str) {
    match task {
        AutonomyTask::ExactDuplicates => (
            "Exact duplicates",
            "Records that share the same DOI or PubMed ID.",
        ),
        AutonomyTask::FuzzyDuplicates => (
            "Similar records",
            "Records with near-identical titles, authors and year but no shared ID.",
        ),
        AutonomyTask::TitleAbstractScreening => (
            "Title and abstract screening",
            "First-pass include or exclude decisions.",
        ),
        AutonomyTask::FullTextScreening => (
            "Full-text screening",
            "Include or exclude decisions after reading the full text.",
        ),
        AutonomyTask::Extraction => (
            "Data extraction",
            "Filling the extraction sheet from the full text.",
        ),
        AutonomyTask::Appraisal => (
            "Quality appraisal",
            "Pre-filling risk-of-bias and quality answers.",
        ),
        AutonomyTask::StudyGrouping => (
            "Grouping reports into studies",
            "Deciding which reports describe the same study.",
        ),
    }
}

fn locked_text(key: &str) -> (&'static str, &'static str) {
    match key {
        "protocol_publishing" => (
            "Publishing the protocol",
            "Publishing a protocol version is always your decision.",
        ),
        _ => (
            "Final exclusion of a study",
            "Finally excluding a study is always your decision.",
        ),
    }
}

fn task_label(task: &str) -> String {
    AutonomyTask::parse(task).map_or_else(
        || match task {
            "assistant_plan" => "Assistant".to_owned(),
            "workflow" => "Automation".to_owned(),
            "study_classification" => "Study design classification".to_owned(),
            other => other.replace('_', " "),
        },
        |task| task_text(task).0.to_owned(),
    )
}

async fn autonomy_dto(state: &AppState, project_id: Uuid) -> Result<AiAutonomyDto, ApiError> {
    let settings = deepref_postgres::get_autonomy_settings(&state.pool, project_id)
        .await
        .map_err(map_autonomy_error)?;
    Ok(AiAutonomyDto {
        tasks: settings
            .into_iter()
            .map(|setting| {
                let (label, description) = task_text(setting.task);
                AiAutonomyTaskDto {
                    task: setting.task.as_str().to_owned(),
                    label: label.to_owned(),
                    description: description.to_owned(),
                    level: setting.level.as_str().to_owned(),
                    default_level: setting.task.default_level().as_str().to_owned(),
                    allowed_levels: [
                        AutonomyLevel::Off,
                        AutonomyLevel::Suggest,
                        AutonomyLevel::SecondReviewer,
                        AutonomyLevel::Act,
                    ]
                    .into_iter()
                    .filter(|level| setting.task.allows(*level))
                    .map(|level| level.as_str().to_owned())
                    .collect(),
                    is_default: setting.is_default,
                }
            })
            .collect(),
        locked: LOCKED_TASKS
            .into_iter()
            .map(|key| {
                let (label, description) = locked_text(key);
                AiLockedTaskDto {
                    task: key.to_owned(),
                    label: label.to_owned(),
                    description: description.to_owned(),
                }
            })
            .collect(),
    })
}

fn map_autonomy_error(error: deepref_postgres::AutonomyStoreError) -> ApiError {
    match error {
        deepref_postgres::AutonomyStoreError::ProjectNotFound => {
            ApiError::NotFound("project not found".to_owned())
        }
        deepref_postgres::AutonomyStoreError::LevelNotAllowed => {
            ApiError::BadRequest("That level is not available for this task.".to_owned())
        }
        deepref_postgres::AutonomyStoreError::Database(error) => ApiError::Database(error),
    }
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/ai/autonomy",
    operation_id = "getAiAutonomy",
    tag = "ai",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    responses(
        (status = 200, description = "How far the AI may go per task", body = AiAutonomyDto),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_ai_autonomy(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<AiAutonomyDto>, ApiError> {
    Ok(Json(autonomy_dto(&state, project_id).await?))
}

#[utoipa::path(
    put,
    path = "/projects/{project_id}/ai/autonomy",
    operation_id = "updateAiAutonomy",
    tag = "ai",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    request_body = UpdateAiAutonomyRequest,
    responses(
        (status = 200, description = "Updated settings", body = AiAutonomyDto),
        (status = 400, description = "Unknown, locked or unavailable setting", body = ErrorResponse),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn update_ai_autonomy(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<UpdateAiAutonomyRequest>,
) -> Result<Json<AiAutonomyDto>, ApiError> {
    let actor = extract_actor(&headers)?;
    let mut parsed = Vec::new();
    for change in &body.changes {
        if LOCKED_TASKS.contains(&change.task.as_str()) {
            return Err(ApiError::BadRequest(
                "This is always your decision and cannot be automated.".to_owned(),
            ));
        }
        let task = AutonomyTask::parse(&change.task)
            .ok_or_else(|| ApiError::BadRequest("Unknown task.".to_owned()))?;
        let level = AutonomyLevel::parse(&change.level)
            .ok_or_else(|| ApiError::BadRequest("Unknown level.".to_owned()))?;
        if !task.allows(level) {
            return Err(ApiError::BadRequest(
                "That level is not available for this task.".to_owned(),
            ));
        }
        parsed.push((task, level));
    }
    for (task, level) in parsed {
        deepref_postgres::set_autonomy_level(&state.pool, project_id, task, level, &actor)
            .await
            .map_err(map_autonomy_error)?;
    }
    Ok(Json(autonomy_dto(&state, project_id).await?))
}

// ---------------------------------------------------------------------------
// Activity feed
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ActivityAffectedDto {
    /// `report`, `record`, `study` or `extraction_value`.
    pub kind: String,
    pub id: Option<Uuid>,
    pub label: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ActivityEvidenceDto {
    pub label: String,
    pub quote: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AiActivityDto {
    pub id: Uuid,
    /// `ai`, `automation` or `assistant`.
    pub actor_type: String,
    pub actor_label: String,
    pub task: String,
    pub task_label: String,
    pub action: String,
    pub summary: String,
    pub affected: Vec<ActivityAffectedDto>,
    pub undoable: bool,
    pub batch_id: Option<Uuid>,
    pub ai_run_id: Option<Uuid>,
    pub proposal_id: Option<Uuid>,
    pub model: Option<String>,
    pub prompt_version: Option<String>,
    pub evidence: Vec<ActivityEvidenceDto>,
    pub created_at: DateTime<Utc>,
    pub undone_at: Option<DateTime<Utc>>,
    pub undone_by: Option<String>,
}

fn activity_dto(record: deepref_postgres::ActivityRecord) -> AiActivityDto {
    let text = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_default()
    };
    AiActivityDto {
        id: record.id,
        actor_type: record.actor_type,
        actor_label: record.actor_label,
        task_label: task_label(&record.task),
        task: record.task,
        action: record.action,
        summary: record.summary,
        affected: record
            .affected
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| ActivityAffectedDto {
                        kind: text(item, "type"),
                        id: item
                            .get("id")
                            .and_then(Value::as_str)
                            .and_then(|id| Uuid::parse_str(id).ok()),
                        label: text(item, "label"),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        undoable: record.undoable,
        batch_id: record.batch_id,
        ai_run_id: record.ai_run_id,
        proposal_id: record.proposal_id,
        model: record.model,
        prompt_version: record.prompt_version,
        evidence: evidence_dtos(&record.evidence),
        created_at: record.created_at,
        undone_at: record.undone_at,
        undone_by: record.undone_by,
    }
}

fn evidence_dtos(value: &Value) -> Vec<ActivityEvidenceDto> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| ActivityEvidenceDto {
                    label: item
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    quote: item
                        .get("quote")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Debug, Deserialize, IntoParams)]
pub(crate) struct AiActivityListParams {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    /// Task key filter, for example `extraction`.
    pub task: Option<String>,
    /// `ai`, `automation` or `assistant`.
    pub actor_type: Option<String>,
    pub batch_id: Option<Uuid>,
    /// True for undone entries only, false for entries still in effect.
    pub undone: Option<bool>,
}

fn map_activity_error(error: deepref_postgres::ActivityError) -> ApiError {
    use deepref_postgres::ActivityError as E;
    match error {
        E::NotFound => ApiError::NotFound("activity entry not found".to_owned()),
        E::AlreadyUndone => ApiError::Conflict {
            code: "activity_already_undone".to_owned(),
            message: "This action was already undone.".to_owned(),
            details: Value::Null,
        },
        E::NotUndoable => ApiError::Conflict {
            code: "activity_not_undoable".to_owned(),
            message: "This action cannot be undone.".to_owned(),
            details: Value::Null,
        },
        E::CannotUndo(message) => ApiError::Conflict {
            code: "activity_cannot_undo".to_owned(),
            message,
            details: Value::Null,
        },
        E::Database(error) => ApiError::Database(error),
    }
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/ai/activity",
    operation_id = "listAiActivity",
    tag = "ai",
    params(("project_id" = Uuid, Path, description = "Project identifier"), AiActivityListParams),
    responses(
        (status = 200, description = "What the AI, automations and the assistant did", body = PaginatedResponse<AiActivityDto>),
        (status = 400, description = "Invalid pagination", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_ai_activity(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Query(params): Query<AiActivityListParams>,
) -> Result<Json<PaginatedResponse<AiActivityDto>>, ApiError> {
    let pagination = PaginationParams {
        cursor: params.cursor.clone(),
        limit: params.limit,
    };
    let limit = pagination.limit()?;
    let cursor = pagination.decode::<(DateTime<Utc>, Uuid)>()?;
    let records = deepref_postgres::list_activity(
        &state.pool,
        project_id,
        deepref_postgres::ActivityFilters {
            task: params.task.as_deref(),
            actor_type: params.actor_type.as_deref(),
            batch_id: params.batch_id,
            undone: params.undone,
        },
        cursor,
        limit,
    )
    .await
    .map_err(map_activity_error)?;
    let items = records.into_iter().map(activity_dto).collect::<Vec<_>>();
    Ok(Json(page(items, limit as usize, |item| {
        (item.created_at, item.id)
    })?))
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AiActivityOverviewDto {
    /// AI-entered extraction values still waiting for a person.
    pub to_verify: i64,
    /// Disagreements between a person and the AI second reviewer.
    pub open_conflicts: i64,
    /// Actions that can still be undone.
    pub undoable: i64,
    /// Automatic AI second review at title/abstract: `not_enabled`, `automatic`,
    /// `needs_calibration` (no passing calibration yet) or `calibration_stale` (the
    /// passing calibration was made for an earlier compiled review and is refused).
    pub second_review_title_abstract: String,
    /// The same state at full text.
    pub second_review_full_text: String,
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/ai/activity/overview",
    operation_id = "getAiActivityOverview",
    tag = "ai",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    responses(
        (status = 200, description = "Counts that need attention", body = AiActivityOverviewDto),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_ai_activity_overview(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<AiActivityOverviewDto>, ApiError> {
    let overview = deepref_postgres::activity_overview(&state.pool, project_id).await?;
    let title_abstract = deepref_postgres::second_review_status(
        &state.pool,
        project_id,
        AutonomyTask::TitleAbstractScreening,
    )
    .await
    .map_err(ApiError::Internal)?;
    let full_text = deepref_postgres::second_review_status(
        &state.pool,
        project_id,
        AutonomyTask::FullTextScreening,
    )
    .await
    .map_err(ApiError::Internal)?;
    Ok(Json(AiActivityOverviewDto {
        to_verify: overview.to_verify,
        open_conflicts: overview.open_conflicts,
        undoable: overview.undoable,
        second_review_title_abstract: title_abstract.as_str().to_owned(),
        second_review_full_text: full_text.as_str().to_owned(),
    }))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/ai/activity/{activity_id}/undo",
    operation_id = "undoAiActivity",
    tag = "ai",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("activity_id" = Uuid, Path, description = "Activity entry identifier")
    ),
    responses(
        (status = 200, description = "The entry after it was undone", body = AiActivityDto),
        (status = 404, description = "Entry not found", body = ErrorResponse),
        (status = 409, description = "Already undone or no longer safe to undo", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn undo_ai_activity(
    State(state): State<AppState>,
    Path((project_id, activity_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<AiActivityDto>, ApiError> {
    let actor = extract_actor(&headers)?;
    let record = deepref_postgres::undo_activity(&state.pool, project_id, activity_id, &actor)
        .await
        .map_err(map_activity_error)?;
    Ok(Json(activity_dto(record)))
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct BatchUndoFailureDto {
    pub activity_id: Uuid,
    pub message: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct BatchUndoDto {
    pub undone: u32,
    /// Entries that could not be undone safely; they were left as they are.
    pub failed: Vec<BatchUndoFailureDto>,
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/ai/activity/batches/{batch_id}/undo",
    operation_id = "undoAiActivityBatch",
    tag = "ai",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("batch_id" = Uuid, Path, description = "Batch identifier")
    ),
    responses(
        (status = 200, description = "How many entries were undone", body = BatchUndoDto),
        (status = 404, description = "Batch not found", body = ErrorResponse),
        (status = 409, description = "Everything in the batch was already undone", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn undo_ai_activity_batch(
    State(state): State<AppState>,
    Path((project_id, batch_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<BatchUndoDto>, ApiError> {
    let actor = extract_actor(&headers)?;
    let result = deepref_postgres::undo_batch(&state.pool, project_id, batch_id, &actor)
        .await
        .map_err(map_activity_error)?;
    Ok(Json(BatchUndoDto {
        undone: result.undone,
        failed: result
            .failed
            .into_iter()
            .map(|(activity_id, message)| BatchUndoFailureDto {
                activity_id,
                message,
            })
            .collect(),
    }))
}

// ---------------------------------------------------------------------------
// Second reviewer
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AiReviewerDecisionDto {
    pub id: Uuid,
    pub report_id: Uuid,
    /// `title_abstract` or `full_text`.
    pub stage: String,
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    /// `include`, `exclude` or `maybe`.
    pub ai_decision: String,
    pub ai_rationale: String,
    pub ai_evidence: Vec<ActivityEvidenceDto>,
    pub ai_model: Option<String>,
    pub human_decision: Option<String>,
    pub human_notes: Option<String>,
    pub human_actor: Option<String>,
    /// `waiting`, `concordant`, `conflict` or `resolved`.
    pub status: String,
    /// `kept_human`, `adopted_ai` or `other`.
    pub resolution: Option<String>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

fn reviewer_dto(record: deepref_postgres::ReviewerDecisionRecord) -> AiReviewerDecisionDto {
    AiReviewerDecisionDto {
        id: record.id,
        report_id: record.report_id,
        stage: record.stage,
        title: record.title,
        abstract_text: record.abstract_text,
        ai_decision: record.ai_decision,
        ai_rationale: record.ai_rationale,
        ai_evidence: evidence_dtos(&record.ai_evidence),
        ai_model: record.ai_model,
        human_decision: record.human_decision,
        human_notes: record.human_notes,
        human_actor: record.human_actor,
        status: record.status,
        resolution: record.resolution,
        resolved_at: record.resolved_at,
        created_at: record.created_at,
    }
}

#[derive(Debug, Deserialize, IntoParams)]
pub(crate) struct AiReviewerListParams {
    /// `title_abstract` or `full_text`.
    pub stage: Option<String>,
    /// `waiting`, `concordant`, `conflict` or `resolved`.
    pub status: Option<String>,
    /// Maximum number of rows, 1 through 500.
    pub limit: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/ai/reviewer-decisions",
    operation_id = "listAiReviewerDecisions",
    tag = "ai",
    params(("project_id" = Uuid, Path, description = "Project identifier"), AiReviewerListParams),
    responses(
        (status = 200, description = "The AI second reviewer's opinions next to the human decisions", body = [AiReviewerDecisionDto]),
        (status = 400, description = "Invalid filter", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_ai_reviewer_decisions(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Query(params): Query<AiReviewerListParams>,
) -> Result<Json<Vec<AiReviewerDecisionDto>>, ApiError> {
    if params
        .stage
        .as_deref()
        .is_some_and(|stage| !matches!(stage, "title_abstract" | "full_text"))
        || params.status.as_deref().is_some_and(|status| {
            !matches!(status, "waiting" | "concordant" | "conflict" | "resolved")
        })
    {
        return Err(ApiError::BadRequest("invalid filter".to_owned()));
    }
    let records = deepref_postgres::list_reviewer_decisions(
        &state.pool,
        project_id,
        params.stage.as_deref(),
        params.status.as_deref(),
        params.limit.unwrap_or(200).clamp(1, 500),
    )
    .await?;
    Ok(Json(records.into_iter().map(reviewer_dto).collect()))
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ResolveReviewerConflictRequest {
    /// The final decision: `include`, `exclude` or `maybe`.
    pub decision: String,
    /// Required when excluding at full-text stage.
    pub exclusion_reason_id: Option<Uuid>,
    pub note: Option<String>,
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/ai/reviewer-decisions/{decision_id}/resolve",
    operation_id = "resolveAiReviewerConflict",
    tag = "ai",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("decision_id" = Uuid, Path, description = "AI reviewer decision identifier")
    ),
    request_body = ResolveReviewerConflictRequest,
    responses(
        (status = 200, description = "The settled conflict", body = AiReviewerDecisionDto),
        (status = 400, description = "Invalid decision", body = ErrorResponse),
        (status = 404, description = "Not found", body = ErrorResponse),
        (status = 409, description = "Already resolved or the screening state changed", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn resolve_ai_reviewer_conflict(
    State(state): State<AppState>,
    Path((project_id, decision_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(body): Json<ResolveReviewerConflictRequest>,
) -> Result<Json<AiReviewerDecisionDto>, ApiError> {
    let actor = extract_actor(&headers)?;
    let decision = match body.decision.as_str() {
        "include" => ScreeningDecision::Include,
        "exclude" => ScreeningDecision::Exclude,
        "maybe" => ScreeningDecision::Maybe,
        _ => {
            return Err(ApiError::BadRequest(
                "decision must be include, exclude or maybe".to_owned(),
            ));
        }
    };
    let record = deepref_postgres::resolve_reviewer_conflict(
        &state.pool,
        project_id,
        decision_id,
        deepref_postgres::ResolveConflict {
            decision,
            exclusion_reason_id: body.exclusion_reason_id,
            note: body.note.filter(|note| !note.trim().is_empty()),
        },
        &actor,
    )
    .await
    .map_err(|error| match error {
        deepref_postgres::ReviewerError::NotFound => {
            ApiError::NotFound("reviewer decision not found".to_owned())
        }
        deepref_postgres::ReviewerError::AlreadyResolved => ApiError::Conflict {
            code: "conflict_already_resolved".to_owned(),
            message: "This conflict was already resolved.".to_owned(),
            details: Value::Null,
        },
        deepref_postgres::ReviewerError::NoProtocol => ApiError::Conflict {
            code: "protocol_not_published".to_owned(),
            message: "Publish the protocol before screening.".to_owned(),
            details: Value::Null,
        },
        deepref_postgres::ReviewerError::Screening(error) => match error {
            deepref_postgres::ScreeningError::Database(error) => ApiError::Database(error),
            deepref_postgres::ScreeningError::RevisionConflict { .. } => ApiError::Conflict {
                code: "screening_revision_conflict".to_owned(),
                message: "The screening decision changed in the meantime. Reload and try again."
                    .to_owned(),
                details: json!({}),
            },
            other => ApiError::BadRequest(other.to_string()),
        },
        deepref_postgres::ReviewerError::Database(error) => ApiError::Database(error),
    })?;
    Ok(Json(reviewer_dto(record)))
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AiStageAgreementDto {
    pub stage: String,
    /// Decisions made by both a person and the AI.
    pub compared: i64,
    pub agreed: i64,
    /// Cohen's kappa; absent until there is enough variation to compute it.
    pub kappa: Option<f64>,
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/ai/reviewer-agreement",
    operation_id = "getAiReviewerAgreement",
    tag = "ai",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    responses(
        (status = 200, description = "Human versus AI agreement per stage", body = [AiStageAgreementDto]),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_ai_reviewer_agreement(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<Vec<AiStageAgreementDto>>, ApiError> {
    let stages = deepref_postgres::reviewer_agreement(&state.pool, project_id).await?;
    Ok(Json(
        stages
            .into_iter()
            .map(|(stage, agreement)| AiStageAgreementDto {
                stage,
                compared: agreement.compared,
                agreed: agreement.agreed,
                kappa: agreement.kappa,
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// Extraction: confirm an AI-entered value
// ---------------------------------------------------------------------------

#[utoipa::path(
    post,
    path = "/projects/{project_id}/studies/{study_id}/extraction/values/{value_id}/confirm",
    operation_id = "confirmExtractionValue",
    tag = "extraction",
    params(
        ("project_id" = Uuid, Path),
        ("study_id" = Uuid, Path),
        ("value_id" = Uuid, Path)
    ),
    responses(
        (status = 200, description = "The confirmed value", body = ExtractionValueDto),
        (status = 404, description = "Value not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn confirm_extraction_value(
    State(state): State<AppState>,
    Path((project_id, study_id, value_id)): Path<(Uuid, Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<Json<ExtractionValueDto>, ApiError> {
    let actor = extract_actor(&headers)?;
    let record =
        deepref_postgres::confirm_value(&state.pool, project_id, study_id, value_id, &actor)
            .await
            .map_err(map_extraction_error)?;
    Ok(Json(value_dto(record)))
}
