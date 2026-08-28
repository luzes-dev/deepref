use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use chrono::{DateTime, Utc};
use deepref_application::{
    GetScreeningQueueQuery, PrismaProjection, ScreenReportCommand, ScreeningQueueSort,
    ScreeningQueueStatus, UndoScreeningCommand,
};
use deepref_domain::{Actor as DomainActor, ActorKind, ScreeningDecision, ScreeningStage};
use deepref_postgres::{
    ScreeningError, ScreeningHistory as PersistenceScreeningHistory,
    ScreeningHistoryItem as PersistenceScreeningHistoryItem,
    ScreeningQueue as PersistenceScreeningQueue,
    ScreeningQueueItem as PersistenceScreeningQueueItem, ScreeningStateSnapshot,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};

const ACTOR_KIND_HEADER: &str = "x-actor-kind";
const ACTOR_ID_HEADER: &str = "x-actor-id";

pub(crate) type Actor = DomainActor;

pub(crate) fn extract_actor(headers: &HeaderMap) -> Result<Actor, ApiError> {
    let kind = headers
        .get(ACTOR_KIND_HEADER)
        .map(|value| {
            value
                .to_str()
                .map(str::to_owned)
                .map_err(|_| ApiError::BadRequest("x-actor-kind must be valid ASCII".to_owned()))
        })
        .transpose()?
        .unwrap_or_else(|| "user".to_owned());
    let kind = ActorKind::parse(&kind).ok_or_else(|| {
        ApiError::BadRequest("x-actor-kind must be user, automation, or system".to_owned())
    })?;
    let id = headers
        .get(ACTOR_ID_HEADER)
        .map(|value| {
            value
                .to_str()
                .map(str::trim)
                .map(str::to_owned)
                .map_err(|_| ApiError::BadRequest("x-actor-id must be valid ASCII".to_owned()))
        })
        .transpose()?
        .unwrap_or_else(|| "local-user".to_owned());
    DomainActor::new(kind, id).map_err(|error| ApiError::BadRequest(error.to_string()))
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct ScreeningQueueItemDto {
    pub report_id: Uuid,
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    pub doi: Option<String>,
    pub publication_year: Option<i32>,
    pub title_abstract_status: String,
    pub full_text_status: String,
    pub final_status: String,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct ScreeningProgressDto {
    pub total: i64,
    pub screened: i64,
    pub unscreened: i64,
    pub included: i64,
    pub excluded: i64,
    pub maybe: i64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct ScreeningQueueDto {
    pub items: Vec<ScreeningQueueItemDto>,
    pub status: String,
    pub sort: String,
    pub total: i64,
    pub next_cursor: Option<String>,
    pub progress: ScreeningProgressDto,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct ScreeningStateDto {
    pub project_id: Uuid,
    pub report_id: Uuid,
    pub title_abstract_status: String,
    pub full_text_status: String,
    pub full_text_exclusion_reason_id: Option<Uuid>,
    pub final_status: String,
    pub revision: i64,
    pub last_event_id: Option<Uuid>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ScreeningStageInput {
    TitleAbstract,
    FullText,
}

impl ScreeningStageInput {
    fn domain(&self) -> ScreeningStage {
        match self {
            Self::TitleAbstract => ScreeningStage::TitleAbstract,
            Self::FullText => ScreeningStage::FullText,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ScreeningDecisionInput {
    Include,
    Exclude,
    Maybe,
}

impl ScreeningDecisionInput {
    fn domain(&self) -> ScreeningDecision {
        match self {
            Self::Include => ScreeningDecision::Include,
            Self::Exclude => ScreeningDecision::Exclude,
            Self::Maybe => ScreeningDecision::Maybe,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ScreenReportRequest {
    pub stage: ScreeningStageInput,
    pub decision: ScreeningDecisionInput,
    pub protocol_version_id: Uuid,
    pub expected_revision: i64,
    pub exclusion_reason_id: Option<Uuid>,
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct UndoScreeningRequest {
    pub stage: ScreeningStageInput,
    pub protocol_version_id: Uuid,
    pub expected_revision: i64,
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ScreeningQueueParams {
    pub status: Option<String>,
    pub search: Option<String>,
    pub sort: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct ScreeningHistoryDto {
    pub project_id: Uuid,
    pub report_id: Uuid,
    pub items: Vec<ScreeningHistoryItemDto>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct ScreeningHistoryItemDto {
    pub id: Uuid,
    pub event_kind: String,
    pub stage: String,
    pub decision: Option<String>,
    pub notes: Option<String>,
    pub protocol_version_id: Uuid,
    pub actor_kind: String,
    pub actor_id: String,
    pub supersedes_event_id: Option<Uuid>,
    pub undoes_event_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub previous_title_abstract_status: String,
    pub previous_full_text_status: String,
    pub previous_full_text_exclusion_reason_id: Option<Uuid>,
    pub previous_final_status: String,
    pub result_title_abstract_status: String,
    pub result_full_text_status: String,
    pub result_full_text_exclusion_reason_id: Option<Uuid>,
    pub result_final_status: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct PrismaDto {
    pub project_id: Uuid,
    pub screening_high_watermark: u64,
    pub as_of: Option<DateTime<Utc>>,
    pub identified_records: u64,
    pub linked_records: u64,
    pub duplicates_removed: u64,
    pub unresolved_records: u64,
    pub pending_dedupe_proposals: u64,
    pub source_canonical_reports: u64,
    pub manually_created_reports: u64,
    pub screened_records: u64,
    pub title_abstract_excluded: u64,
    pub title_abstract_pending: u64,
    pub reports_sought: u64,
    pub reports_not_retrieved: u64,
    pub full_text_assessed: u64,
    pub full_text_pending: u64,
    pub full_text_included: u64,
    pub full_text_excluded: u64,
    pub full_text_exclusions: Vec<PrismaReasonDto>,
    pub included_reports_not_grouped: u64,
    pub included_studies: u64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct PrismaReasonDto {
    pub id: Uuid,
    pub code: String,
    pub label: String,
    pub count: u64,
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/screening",
    operation_id = "getScreeningQueue",
    tag = "review",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("status" = Option<String>, Query, description = "Title/abstract status filter"),
        ("search" = Option<String>, Query, description = "Title/abstract search"),
        ("sort" = Option<String>, Query, description = "Stable queue sort"),
        ("cursor" = Option<String>, Query, description = "Opaque page cursor"),
        ("limit" = Option<i64>, Query, description = "Maximum rows, 1 through 100")
    ),
    responses(
        (status = 200, description = "Bounded title/abstract screening queue", body = ScreeningQueueDto),
        (status = 400, description = "Invalid queue parameters", body = ErrorResponse),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_screening_queue(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Query(params): Query<ScreeningQueueParams>,
) -> Result<Json<ScreeningQueueDto>, ApiError> {
    let query = queue_query(project_id, params)?;
    let queue = deepref_postgres::get_screening_queue(&state.pool, query)
        .await
        .map_err(map_screening_error)?;
    Ok(Json(queue_dto(queue)))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/screening/title-abstract",
    operation_id = "listTitleAbstractScreeningQueue",
    tag = "review",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("status" = Option<String>, Query, description = "Title/abstract status filter"),
        ("search" = Option<String>, Query, description = "Title/abstract search"),
        ("sort" = Option<String>, Query, description = "Stable queue sort"),
        ("cursor" = Option<String>, Query, description = "Opaque page cursor"),
        ("limit" = Option<i64>, Query, description = "Maximum rows, 1 through 100")
    ),
    responses(
        (status = 200, description = "Bounded title/abstract screening queue", body = ScreeningQueueDto),
        (status = 400, description = "Invalid queue parameters", body = ErrorResponse),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_title_abstract_queue(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Query(params): Query<ScreeningQueueParams>,
) -> Result<Json<ScreeningQueueDto>, ApiError> {
    let query = queue_query(project_id, params)?;
    let queue = deepref_postgres::get_screening_queue(&state.pool, query)
        .await
        .map_err(map_screening_error)?;
    Ok(Json(queue_dto(queue)))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/screening/next",
    operation_id = "getNextScreeningItem",
    tag = "review",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("status" = Option<String>, Query, description = "Title/abstract status filter"),
        ("search" = Option<String>, Query, description = "Title/abstract search"),
        ("sort" = Option<String>, Query, description = "Stable queue sort"),
        ("cursor" = Option<String>, Query, description = "Opaque page cursor")
    ),
    responses(
        (status = 200, description = "Next title/abstract screening item", body = ScreeningQueueItemDto),
        (status = 400, description = "Invalid queue parameters", body = ErrorResponse),
        (status = 404, description = "Queue item or project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_next_screening_item(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Query(params): Query<ScreeningQueueParams>,
) -> Result<Json<ScreeningQueueItemDto>, ApiError> {
    let query = queue_query(project_id, params)?;
    let item = deepref_postgres::get_next_screening_item(&state.pool, query)
        .await
        .map_err(map_screening_error)?;
    Ok(Json(queue_item_dto(item)))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/reports/{report_id}/screening",
    operation_id = "screenReport",
    tag = "review",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("report_id" = Uuid, Path, description = "Report identifier")
    ),
    request_body = ScreenReportRequest,
    responses(
        (status = 200, description = "Current screening state", body = ScreeningStateDto),
        (status = 400, description = "Invalid decision", body = ErrorResponse),
        (status = 404, description = "Report or protocol not found", body = ErrorResponse),
        (status = 409, description = "Revision conflict", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn screen_report(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(input): Json<ScreenReportRequest>,
) -> Result<Json<ScreeningStateDto>, ApiError> {
    let actor = extract_actor(&headers)?;
    let command = ScreenReportCommand {
        project_id: project_id.into(),
        report_id: report_id.into(),
        stage: input.stage.domain(),
        decision: input.decision.domain(),
        exclusion_reason_id: input.exclusion_reason_id.map(Into::into),
        protocol_version_id: input.protocol_version_id.into(),
        expected_revision: input.expected_revision,
        notes: input.notes,
        actor,
    };
    let state = deepref_postgres::screen_report(&state.pool, command)
        .await
        .map_err(map_screening_error)?;
    Ok(Json(state_dto(state)))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/reports/{report_id}/screening/undo",
    operation_id = "undoScreening",
    tag = "review",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("report_id" = Uuid, Path, description = "Report identifier")
    ),
    request_body = UndoScreeningRequest,
    responses(
        (status = 200, description = "Restored screening state", body = ScreeningStateDto),
        (status = 400, description = "Invalid undo", body = ErrorResponse),
        (status = 404, description = "Report or protocol not found", body = ErrorResponse),
        (status = 409, description = "Revision conflict", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn undo_screening(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(input): Json<UndoScreeningRequest>,
) -> Result<Json<ScreeningStateDto>, ApiError> {
    let actor = extract_actor(&headers)?;
    let command = UndoScreeningCommand {
        project_id: project_id.into(),
        report_id: report_id.into(),
        stage: input.stage.domain(),
        protocol_version_id: input.protocol_version_id.into(),
        expected_revision: input.expected_revision,
        notes: input.notes,
        actor,
    };
    let state = deepref_postgres::undo_screening(&state.pool, command)
        .await
        .map_err(map_screening_error)?;
    Ok(Json(state_dto(state)))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/reports/{report_id}/screening/history",
    operation_id = "getScreeningHistory",
    tag = "review",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("report_id" = Uuid, Path, description = "Report identifier")
    ),
    responses(
        (status = 200, description = "Append-only screening history", body = ScreeningHistoryDto),
        (status = 404, description = "Report not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_screening_history(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ScreeningHistoryDto>, ApiError> {
    let history = deepref_postgres::get_screening_history(&state.pool, project_id, report_id)
        .await
        .map_err(map_screening_error)?;
    Ok(Json(history_dto(history)))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/prisma",
    operation_id = "getProjectPrisma",
    tag = "review",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    responses(
        (status = 200, description = "PRISMA projection counts", body = PrismaDto),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_prisma(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<PrismaDto>, ApiError> {
    let projection = deepref_postgres::get_prisma_projection(&state.pool, project_id)
        .await
        .map_err(|error| match error {
            deepref_postgres::PrismaProjectionError::Invariant(error) => {
                ApiError::DataIntegrity(error.to_string())
            }
            deepref_postgres::PrismaProjectionError::NegativeCount { .. } => {
                ApiError::DataIntegrity(error.to_string())
            }
            deepref_postgres::PrismaProjectionError::Database(error) => ApiError::Database(error),
            deepref_postgres::PrismaProjectionError::Json(error) => {
                ApiError::Internal(error.into())
            }
        })?
        .ok_or_else(|| ApiError::NotFound("project not found".to_owned()))?;
    Ok(Json(prisma_dto(projection)))
}

fn prisma_dto(projection: PrismaProjection) -> PrismaDto {
    PrismaDto {
        project_id: projection.project_id,
        screening_high_watermark: projection.screening_high_watermark.get(),
        as_of: projection.as_of,
        identified_records: projection.identified_records.get(),
        linked_records: projection.linked_records.get(),
        duplicates_removed: projection.duplicates_removed.get(),
        unresolved_records: projection.unresolved_records.get(),
        pending_dedupe_proposals: projection.pending_dedupe_proposals.get(),
        source_canonical_reports: projection.source_canonical_reports.get(),
        manually_created_reports: projection.manually_created_reports.get(),
        screened_records: projection.screened_records.get(),
        title_abstract_excluded: projection.title_abstract_excluded.get(),
        title_abstract_pending: projection.title_abstract_pending.get(),
        reports_sought: projection.reports_sought.get(),
        reports_not_retrieved: projection.reports_not_retrieved.get(),
        full_text_assessed: projection.full_text_assessed.get(),
        full_text_pending: projection.full_text_pending.get(),
        full_text_included: projection.full_text_included.get(),
        full_text_excluded: projection.full_text_excluded.get(),
        full_text_exclusions: projection
            .full_text_exclusions
            .into_iter()
            .map(|reason| PrismaReasonDto {
                id: reason.id,
                code: reason.code,
                label: reason.label,
                count: reason.count.get(),
            })
            .collect(),
        included_reports_not_grouped: projection.included_reports_not_grouped.get(),
        included_studies: projection.included_studies.get(),
    }
}

fn queue_query(
    project_id: Uuid,
    params: ScreeningQueueParams,
) -> Result<GetScreeningQueueQuery, ApiError> {
    let status = params.status.as_deref().unwrap_or("unscreened");
    let status = ScreeningQueueStatus::parse(status).ok_or_else(|| {
        ApiError::BadRequest(
            "status must be unscreened, include, exclude, maybe, or all".to_owned(),
        )
    })?;
    let sort = params.sort.as_deref().unwrap_or("created_asc");
    let sort = ScreeningQueueSort::parse(sort).ok_or_else(|| {
        ApiError::BadRequest(
            "sort must be created_asc, created_desc, title_asc, title_desc, year_asc, or year_desc"
                .to_owned(),
        )
    })?;
    let limit = params.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::BadRequest(
            "limit must be between 1 and 100".to_owned(),
        ));
    }
    let search = params.search.map(|value| value.trim().to_owned());
    if search.as_deref().is_some_and(|value| value.len() > 200) {
        return Err(ApiError::BadRequest(
            "search must be at most 200 characters".to_owned(),
        ));
    }
    Ok(GetScreeningQueueQuery {
        project_id: project_id.into(),
        status,
        search: search.filter(|value| !value.is_empty()),
        sort,
        cursor: params.cursor,
        limit,
    })
}

fn queue_dto(queue: PersistenceScreeningQueue) -> ScreeningQueueDto {
    ScreeningQueueDto {
        items: queue.items.into_iter().map(queue_item_dto).collect(),
        status: queue.status,
        sort: queue.sort,
        total: queue.total,
        next_cursor: queue.next_cursor,
        progress: ScreeningProgressDto {
            total: queue.progress.total,
            screened: queue.progress.screened,
            unscreened: queue.progress.unscreened,
            included: queue.progress.included,
            excluded: queue.progress.excluded,
            maybe: queue.progress.maybe,
        },
    }
}

fn queue_item_dto(item: PersistenceScreeningQueueItem) -> ScreeningQueueItemDto {
    ScreeningQueueItemDto {
        report_id: item.report_id,
        title: item.title,
        abstract_text: item.abstract_text,
        doi: item.doi,
        publication_year: item.publication_year,
        title_abstract_status: item.title_abstract_status,
        full_text_status: item.full_text_status,
        final_status: item.final_status,
        revision: item.revision,
    }
}

fn state_dto(state: ScreeningStateSnapshot) -> ScreeningStateDto {
    ScreeningStateDto {
        project_id: state.project_id,
        report_id: state.report_id,
        title_abstract_status: state.title_abstract_status,
        full_text_status: state.full_text_status,
        full_text_exclusion_reason_id: state.full_text_exclusion_reason_id,
        final_status: state.final_status,
        revision: state.revision,
        last_event_id: state.last_event_id,
        updated_at: state.updated_at,
    }
}

fn history_dto(history: PersistenceScreeningHistory) -> ScreeningHistoryDto {
    ScreeningHistoryDto {
        project_id: history.project_id,
        report_id: history.report_id,
        items: history.items.into_iter().map(history_item_dto).collect(),
    }
}

fn history_item_dto(item: PersistenceScreeningHistoryItem) -> ScreeningHistoryItemDto {
    ScreeningHistoryItemDto {
        id: item.id,
        event_kind: item.event_kind,
        stage: item.stage,
        decision: item.decision,
        notes: item.notes,
        protocol_version_id: item.protocol_version_id,
        actor_kind: item.actor_kind,
        actor_id: item.actor_id,
        supersedes_event_id: item.supersedes_event_id,
        undoes_event_id: item.undoes_event_id,
        created_at: item.created_at,
        previous_title_abstract_status: item.previous_title_abstract_status,
        previous_full_text_status: item.previous_full_text_status,
        previous_full_text_exclusion_reason_id: item.previous_full_text_exclusion_reason_id,
        previous_final_status: item.previous_final_status,
        result_title_abstract_status: item.result_title_abstract_status,
        result_full_text_status: item.result_full_text_status,
        result_full_text_exclusion_reason_id: item.result_full_text_exclusion_reason_id,
        result_final_status: item.result_final_status,
    }
}

pub(crate) fn map_screening_error(error: ScreeningError) -> ApiError {
    match error {
        ScreeningError::Database(error) => ApiError::Database(error),
        ScreeningError::ProjectNotFound => ApiError::NotFound("project not found".to_owned()),
        ScreeningError::ReportNotInProject => {
            ApiError::NotFound("report is not part of this project".to_owned())
        }
        ScreeningError::ProtocolNotFound => {
            ApiError::NotFound("published protocol not found".to_owned())
        }
        ScreeningError::ExclusionReasonNotFound => {
            ApiError::BadRequest("exclusion_reason_id does not belong to this project".to_owned())
        }
        ScreeningError::ExclusionReasonWrongStage => ApiError::BadRequest(
            "exclusion_reason_id is for a different screening stage".to_owned(),
        ),
        ScreeningError::RevisionConflict { current } => ApiError::Conflict {
            code: "screening_revision_conflict".to_owned(),
            message: "screening state changed; refresh before saving".to_owned(),
            details: json!({
                "currentRevision": current.revision,
                "currentState": current,
            }),
        },
        ScreeningError::Repeated { current } => ApiError::Conflict {
            code: "screening_decision_repeated".to_owned(),
            message: "screening decision is already current".to_owned(),
            details: json!({
                "currentRevision": current.revision,
                "currentState": current,
            }),
        },
        ScreeningError::NoHistory => {
            ApiError::BadRequest("screening history has no event to undo".to_owned())
        }
        ScreeningError::UndoNotLatest { current } => ApiError::Conflict {
            code: "screening_undo_not_latest".to_owned(),
            message: "only the latest screening event can be undone".to_owned(),
            details: json!({
                "currentRevision": current.revision,
                "currentState": current,
            }),
        },
        ScreeningError::HistoryNotFound => {
            ApiError::NotFound("screening history is unavailable".to_owned())
        }
        ScreeningError::QueueEmpty => ApiError::NotFound("screening queue is empty".to_owned()),
        ScreeningError::InvalidCursor => {
            ApiError::BadRequest("invalid screening cursor".to_owned())
        }
        ScreeningError::InvalidData(message) | ScreeningError::InvalidTransition(message) => {
            ApiError::BadRequest(message)
        }
    }
}
