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
    pub state: ProjectionStateDto,
    pub requested_revision: i64,
    pub applied_revision: i64,
    pub revision: i64,
    pub watermark: i64,
    pub lag: i64,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub rebuild_state: Option<ProjectionRebuildStateDto>,
}

#[derive(Debug, Serialize, serde::Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionStateDto {
    Pending,
    Ready,
    Lagging,
    Rebuilding,
    Failed,
}
impl ProjectionStateDto {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Lagging => "lagging",
            Self::Rebuilding => "rebuilding",
            Self::Failed => "failed",
        }
    }
}
impl From<deepref_domain::graph::ProjectionState> for ProjectionStateDto {
    fn from(state: deepref_domain::graph::ProjectionState) -> Self {
        use deepref_domain::graph::ProjectionState as State;
        match state {
            State::Pending => Self::Pending,
            State::Ready => Self::Ready,
            State::Lagging => Self::Lagging,
            State::Rebuilding => Self::Rebuilding,
            State::Failed => Self::Failed,
        }
    }
}
#[derive(Debug, Serialize, serde::Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionRebuildStateDto {
    Rebuilding,
}

impl From<deepref_app::ProjectionStatus> for ProjectionStatusDto {
    fn from(status: deepref_app::ProjectionStatus) -> Self {
        Self {
            project_id: status.project_id,
            state: status.state.into(),
            requested_revision: status.requested_revision,
            applied_revision: status.applied_revision,
            revision: status.revision,
            watermark: status.watermark,
            lag: status.lag,
            last_success_at: status.last_success_at,
            last_error: status.last_error,
            rebuild_state: status.rebuild_state.map(|state| match state {
                deepref_domain::graph::ProjectionRebuildState::Rebuilding => {
                    ProjectionRebuildStateDto::Rebuilding
                }
            }),
        }
    }
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

    Ok(Json(status.into()))
}
