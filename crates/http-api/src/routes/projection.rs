use axum::{
    Json,
    extract::{Path, State},
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::Row;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectionStatusDto {
    pub project_id: Uuid,
    pub state: String,
    pub revision: i64,
    pub watermark: i64,
    pub lag: i64,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub rebuild_state: Option<String>,
}

#[utoipa::path(get, path="/projects/{project_id}/projection", operation_id="getProjectProjection", tag="projection",
    params(("project_id"=Uuid, Path)),
    responses((status=200,body=ProjectionStatusDto),(status=404,body=ErrorResponse)))]
pub async fn get_projection(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<ProjectionStatusDto>, ApiError> {
    let row = sqlx::query(
        "SELECT state,revision,watermark,lag,last_success_at,last_error,rebuild_state \
         FROM projection_state WHERE projection_name='postgres_graph' AND (project_id=$1 OR project_id IS NULL) \
         ORDER BY project_id NULLS LAST LIMIT 1",
    ).bind(project_id).fetch_one(&state.pool).await?;
    Ok(Json(ProjectionStatusDto {
        project_id,
        state: row.get("state"),
        revision: row.get("revision"),
        watermark: row.get("watermark"),
        lag: row.get("lag"),
        last_success_at: row.get("last_success_at"),
        last_error: row.get("last_error"),
        rebuild_state: row.get("rebuild_state"),
    }))
}
