use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chrono::{DateTime, Utc};
use deepref_domain::{ProjectId, ScientificAssignmentRole, UserId, WorkspaceId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    auth::CurrentSession,
    authz::require_for_handler,
    error::{ApiError, ErrorResponse},
    state::AppState,
};

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct ScientificAssignmentDto {
    project_id: Uuid,
    user_id: Uuid,
    role: String,
    assigned_by: Uuid,
    created_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct AssignScientificRoleRequest {
    user_id: Uuid,
    role: String,
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/scientific-assignments",
    operation_id = "listScientificAssignments",
    tag = "projects",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    responses(
        (status = 200, description = "Active scientific assignments", body = [ScientificAssignmentDto]),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_scientific_assignments_route(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<Vec<ScientificAssignmentDto>>, ApiError> {
    let assignments =
        deepref_app::list_scientific_assignments(&state.pool, ProjectId::new(project_id))
            .await
            .map_err(map_assignment_error)?;
    Ok(Json(assignments.into_iter().map(assignment_dto).collect()))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/scientific-assignments",
    operation_id = "assignScientificRole",
    tag = "projects",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    request_body = AssignScientificRoleRequest,
    responses(
        (status = 201, description = "Scientific assignment created or already active", body = ScientificAssignmentDto),
        (status = 400, description = "Unknown scientific role", body = ErrorResponse),
        (status = 404, description = "Project or user not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn assign_scientific_role(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    session: Option<CurrentSession>,
    request_actor: crate::routes::actor::AuthenticatedActor,
    Json(input): Json<AssignScientificRoleRequest>,
) -> Result<(StatusCode, Json<ScientificAssignmentDto>), ApiError> {
    let role = ScientificAssignmentRole::parse(&input.role)
        .ok_or_else(|| ApiError::BadRequest("unknown scientific assignment role".to_owned()))?;
    require_workspace_manager(&state, session.as_ref(), project_id).await?;
    let assignment = deepref_app::assign_scientific_reviewer(
        &state.pool,
        ProjectId::new(project_id),
        UserId::new(input.user_id),
        role,
        request_actor.user_id,
    )
    .await
    .map_err(map_assignment_error)?;
    Ok((StatusCode::CREATED, Json(assignment_dto(assignment))))
}

#[utoipa::path(
    delete,
    path = "/projects/{project_id}/scientific-assignments/{user_id}/{role}",
    operation_id = "revokeScientificRole",
    tag = "projects",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("user_id" = Uuid, Path, description = "Assigned user"),
        ("role" = String, Path, description = "Scientific role")
    ),
    responses(
        (status = 200, description = "Revoked scientific assignment", body = ScientificAssignmentDto),
        (status = 400, description = "Unknown scientific role", body = ErrorResponse),
        (status = 404, description = "Project or active assignment not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn revoke_scientific_role(
    State(state): State<AppState>,
    Path((project_id, user_id, role)): Path<(Uuid, Uuid, String)>,
    _request_actor: crate::routes::actor::AuthenticatedActor,
    session: Option<CurrentSession>,
) -> Result<Json<ScientificAssignmentDto>, ApiError> {
    let role = ScientificAssignmentRole::parse(&role)
        .ok_or_else(|| ApiError::BadRequest("unknown scientific assignment role".to_owned()))?;
    require_workspace_manager(&state, session.as_ref(), project_id).await?;
    let assignment = deepref_app::revoke_scientific_assignment(
        &state.pool,
        ProjectId::new(project_id),
        UserId::new(user_id),
        role,
    )
    .await
    .map_err(map_assignment_error)?;
    Ok(Json(assignment_dto(assignment)))
}

async fn require_workspace_manager(
    state: &AppState,
    session: Option<&CurrentSession>,
    project_id: Uuid,
) -> Result<(), ApiError> {
    let workspace_id: Option<Uuid> =
        sqlx::query_scalar("SELECT workspace_id FROM projects WHERE id=$1")
            .bind(project_id)
            .fetch_optional(&state.pool)
            .await?;
    let workspace_id =
        workspace_id.ok_or_else(|| ApiError::NotFound("project not found".to_owned()))?;
    let context = session.map(|CurrentSession(context)| context);
    require_for_handler(
        state,
        context,
        deepref_domain::authorization::Permission::WorkspaceManage,
        deepref_domain::authorization::Resource::Workspace(WorkspaceId::new(workspace_id)),
    )
    .await
}

fn assignment_dto(assignment: deepref_app::ScientificAssignment) -> ScientificAssignmentDto {
    ScientificAssignmentDto {
        project_id: assignment.project_id.as_uuid(),
        user_id: assignment.user_id.as_uuid(),
        role: assignment.role.as_str().to_owned(),
        assigned_by: assignment.assigned_by.as_uuid(),
        created_at: assignment.created_at,
        revoked_at: assignment.revoked_at,
    }
}

fn map_assignment_error(error: deepref_app::ScientificAssignmentError) -> ApiError {
    match error {
        deepref_app::ScientificAssignmentError::Database(error) => ApiError::Database(error),
        deepref_app::ScientificAssignmentError::NotFound => {
            ApiError::NotFound("assignment or active user not found".to_owned())
        }
        deepref_app::ScientificAssignmentError::UnknownStoredRole(role) => {
            ApiError::Internal(anyhow::anyhow!("unknown scientific role {role:?}"))
        }
    }
}
