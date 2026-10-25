use axum::{
    Json,
    extract::{Path, State},
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};

#[derive(Debug, Serialize, serde::Deserialize, ToSchema)]
pub struct ProjectionStatusDto {
    pub project_id: Uuid,
    pub state: String,
    pub requested_revision: i64,
    pub applied_revision: i64,
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
    let status = deepref_app::get_projection_status(&state.pool, project_id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or_else(|| ApiError::NotFound("project not found".to_owned()))?;

    Ok(Json(ProjectionStatusDto {
        project_id: status.project_id,
        state: status.state.as_str().to_owned(),
        requested_revision: status.requested_revision,
        applied_revision: status.applied_revision,
        revision: status.revision,
        watermark: status.watermark,
        lag: status.lag,
        last_success_at: status.last_success_at,
        last_error: status.last_error,
        rebuild_state: status.rebuild_state,
    }))
}
