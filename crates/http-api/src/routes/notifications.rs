use axum::{
    Json,
    extract::{Query, State},
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use deepref_postgres::{list_notifications, mark_notifications_read, unread_summary};

use super::pagination::{PaginatedResponse, PaginationParams, page};
use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct NotificationDto {
    pub id: Uuid,
    pub revision: i64,
    pub kind: String,
    pub severity: String,
    pub project_id: Option<Uuid>,
    pub title: String,
    pub body: Option<String>,
    #[schema(value_type = Object)]
    pub payload: serde_json::Value,
    pub read_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct NotificationUnreadDto {
    pub count: i64,
    pub latest_revision: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct MarkNotificationsReadRequest {
    /// Explicit notification ids to mark read. Ignored when `all` is true.
    #[schema(value_type = Option<Vec<Uuid>>)]
    pub ids: Option<Vec<Uuid>>,
    /// Mark every notification read.
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct MarkNotificationsReadResponse {
    pub updated: i64,
}

#[derive(Debug, Deserialize, IntoParams)]
pub(crate) struct ListNotificationsParams {
    /// Opaque cursor returned by the previous page.
    pub cursor: Option<String>,
    /// Page size from 1 through 100.
    pub limit: Option<i64>,
}

fn dto_from_record(record: deepref_postgres::NotificationRecord) -> NotificationDto {
    NotificationDto {
        id: record.id,
        revision: record.revision,
        kind: record.kind,
        severity: record.severity,
        project_id: record.project_id,
        title: record.title,
        body: record.body,
        payload: record.payload,
        read_at: record.read_at,
        created_at: record.created_at,
    }
}

#[utoipa::path(
    get,
    path = "/notifications",
    operation_id = "listNotifications",
    tag = "notifications",
    responses(
        (status = 200, description = "Notifications ordered newest first", body = PaginatedResponse<NotificationDto>),
        (status = 400, description = "Invalid pagination", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_notifications_route(
    State(state): State<AppState>,
    Query(params): Query<ListNotificationsParams>,
) -> Result<Json<PaginatedResponse<NotificationDto>>, ApiError> {
    let pagination = PaginationParams {
        cursor: params.cursor,
        limit: params.limit,
    };
    let limit = pagination.limit()?;
    let cursor: Option<i64> = pagination.decode()?;
    let result = list_notifications(&state.pool, cursor, limit).await?;
    Ok(Json(page(
        result.items.into_iter().map(dto_from_record).collect(),
        limit as usize,
        |item| item.revision,
    )?))
}

#[utoipa::path(
    get,
    path = "/notifications/unread-count",
    operation_id = "getUnreadNotificationCount",
    tag = "notifications",
    responses(
        (status = 200, description = "Unread count and newest notification revision", body = NotificationUnreadDto),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_unread_notification_count(
    State(state): State<AppState>,
) -> Result<Json<NotificationUnreadDto>, ApiError> {
    let summary = unread_summary(&state.pool).await?;
    Ok(Json(NotificationUnreadDto {
        count: summary.count,
        latest_revision: summary.latest_revision,
    }))
}

#[utoipa::path(
    post,
    path = "/notifications/mark-read",
    operation_id = "markNotificationsRead",
    tag = "notifications",
    request_body = MarkNotificationsReadRequest,
    responses(
        (status = 200, description = "Notifications transitioned to read", body = MarkNotificationsReadResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn mark_notifications_read_route(
    State(state): State<AppState>,
    Json(input): Json<MarkNotificationsReadRequest>,
) -> Result<Json<MarkNotificationsReadResponse>, ApiError> {
    let updated = mark_notifications_read(
        &state.pool,
        &deepref_postgres::MarkNotificationsRead {
            ids: input.ids,
            all: input.all,
        },
    )
    .await?;
    Ok(Json(MarkNotificationsReadResponse { updated }))
}
