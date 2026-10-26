use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    auth::CurrentSession,
    authz::require_for_handler,
    error::{ApiError, ErrorResponse},
    state::AppState,
};
use deepref_app::identity::AuditRequest;
use deepref_domain::{
    ProjectId, UserId, WorkspaceId,
    authorization::{Permission, Resource},
};

use super::{
    pagination::{PaginatedResponse, PaginationParams, page},
    settings,
};

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct ProjectDto {
    id: Uuid,
    name: String,
    description: Option<String>,
    default_max_depth: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    /// Number of articles (project reports) in the project.
    article_count: i64,
    /// When this displayed count was observed. Cached snapshots are at most five
    /// seconds old; request Cache-Control: no-cache for a primary count.
    article_count_as_of: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateProject {
    name: String,
    description: Option<String>,
    default_max_depth: Option<i32>,
    /// Workspace to create the project in. Optional when the caller belongs to
    /// exactly one workspace that can hold projects; required otherwise. The
    /// caller needs `project:create` there. Ignored when updating.
    workspace_id: Option<Uuid>,
}

impl CreateProject {
    fn validate(mut self) -> Result<Self, ApiError> {
        self.name = self.name.trim().to_owned();
        if self.name.is_empty() {
            return Err(ApiError::BadRequest("name must not be blank".to_owned()));
        }
        if let Some(value) = self.default_max_depth {
            deepref_domain::acquisition::MaxDepth::try_from(value)
                .map_err(|message| ApiError::BadRequest(message.to_owned()))?;
        }
        Ok(self)
    }
}

#[utoipa::path(
    get,
    path = "/projects",
    operation_id = "listProjects",
    tag = "projects",
    params(("Cache-Control" = Option<String>, Header, description = "no-cache, no-store or max-age=0 bypasses the five-second displayed count cache")),
    responses(
        (status = 200, description = "Projects ordered by most recently updated", body = PaginatedResponse<ProjectDto>),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_projects(
    State(state): State<AppState>,
    session: Option<CurrentSession>,
    Query(pagination): Query<PaginationParams>,
    headers: HeaderMap,
) -> Result<Json<PaginatedResponse<ProjectDto>>, ApiError> {
    let limit = pagination.limit()?;
    let cursor: Option<(DateTime<Utc>, Uuid)> = pagination.decode()?;
    // With authentication on, only projects the caller can view now. Without
    // it (local development) every project.
    let viewer = session
        .as_ref()
        .and_then(|session| session.0.principal.user_id());
    let roles = deepref_app::authorization::ProjectViewRoles::current();
    let rows = sqlx::query(concat!(
        "SELECT id,name,description,default_max_depth,created_at,updated_at,workspace_id \
         FROM projects \
         WHERE ($1::timestamptz IS NULL OR (updated_at,id)<($1,$2)) \
           AND ($4::uuid IS NULL OR ",
        deepref_app::viewable_project_predicate!("projects.id", "$4", "$5", "$6"),
        ") ORDER BY updated_at DESC,id DESC LIMIT $3"
    ))
    .bind(cursor.as_ref().map(|value| value.0))
    .bind(cursor.as_ref().map(|value| value.1))
    .bind(limit + 1)
    .bind(viewer.map(UserId::as_uuid))
    .bind(roles.workspace_roles())
    .bind(roles.project_roles())
    .fetch_all(&state.pool)
    .await?;
    let identities = rows
        .iter()
        .map(|row| {
            (
                WorkspaceId::new(row.get("workspace_id")),
                ProjectId::new(row.get("id")),
            )
        })
        .collect::<Vec<_>>();
    let counts = deepref_app::project_counts::article_counts(
        &state.pool,
        &identities,
        state.project_count_cache.as_deref(),
        primary_counts(&headers),
    )
    .await?;
    let projects = rows
        .into_iter()
        .filter_map(|row| {
            let project = ProjectId::new(row.get("id"));
            counts
                .get(&project)
                .map(|count| project_with_count(row, count.count, count.observed_at))
        })
        .collect();
    Ok(Json(page(projects, limit as usize, |project| {
        (project.updated_at, project.id)
    })?))
}

#[utoipa::path(
    post,
    path = "/projects",
    operation_id = "createProject",
    tag = "projects",
    request_body = CreateProject,
    responses(
        (status = 201, description = "Project created", body = ProjectDto),
        (status = 400, description = "Invalid project", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn create_project(
    State(state): State<AppState>,
    session: Option<CurrentSession>,
    Json(input): Json<CreateProject>,
) -> Result<(StatusCode, Json<ProjectDto>), ApiError> {
    let workspace = target_workspace(&state, session.as_ref(), input.workspace_id).await?;
    let input = input.validate()?;
    settings::ensure_settings(&state.pool).await?;
    let default_max_depth = match input.default_max_depth {
        Some(value) => value,
        None => {
            let row = sqlx::query("SELECT default_max_depth FROM settings WHERE id = 1")
                .fetch_one(&state.pool)
                .await?;
            row.get("default_max_depth")
        }
    };
    let id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        r#"
        INSERT INTO projects (id, name, description, default_max_depth, workspace_id)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, name, description, default_max_depth, created_at, updated_at,
          0::bigint AS article_count, statement_timestamp() AS article_count_as_of
        "#,
    )
    .bind(id)
    .bind(input.name)
    .bind(input.description)
    .bind(default_max_depth)
    .bind(workspace.as_uuid())
    .fetch_one(&mut *tx)
    .await?;
    let actor = session
        .as_ref()
        .and_then(|CurrentSession(context)| context.principal.user_id());
    deepref_app::record_project_created(
        &mut tx,
        actor,
        workspace,
        ProjectId::new(id),
        AuditRequest {
            request_id: crate::error::correlation_id().as_deref(),
            session_ref: None,
        },
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(project_from_row(row))))
}

/// The workspace a new project goes into, after `project:create` is required
/// on it. A foreign workspace and a missing one are the same `404`. With
/// authentication off (local development) there is no principal and the
/// migration workspace holds the project.
async fn target_workspace(
    state: &AppState,
    session: Option<&CurrentSession>,
    requested: Option<Uuid>,
) -> Result<WorkspaceId, ApiError> {
    let Some(CurrentSession(context)) = session else {
        return Ok(requested.map_or(
            deepref_app::workspaces::MIGRATION_WORKSPACE_ID,
            WorkspaceId::new,
        ));
    };
    let Some(user) = context.principal.user_id() else {
        return Err(ApiError::NotFound("resource not found".to_owned()));
    };
    let workspace = match requested {
        Some(id) => WorkspaceId::new(id),
        None => deepref_app::authorization::sole_project_creation_workspace(&state.pool, user)
            .await?
            .ok_or_else(|| {
                ApiError::BadRequest(
                    "workspace_id is required unless you belong to exactly one workspace"
                        .to_owned(),
                )
            })?,
    };
    require_for_handler(
        state,
        Some(context),
        Permission::ProjectCreate,
        Resource::Workspace(workspace),
    )
    .await?;
    Ok(workspace)
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}",
    operation_id = "getProject",
    tag = "projects",
    params(("project_id" = Uuid, Path, description = "Project identifier"),
        ("Cache-Control" = Option<String>, Header, description = "no-cache, no-store or max-age=0 bypasses the five-second displayed count cache")),
    responses(
        (status = 200, description = "Project", body = ProjectDto),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_project(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<ProjectDto>, ApiError> {
    let row = sqlx::query(
        "SELECT id, name, description, default_max_depth, created_at, updated_at,workspace_id \
         FROM projects WHERE id = $1",
    )
    .bind(project_id)
    .fetch_one(&state.pool)
    .await?;
    let project = ProjectId::new(project_id);
    let workspace = WorkspaceId::new(row.get("workspace_id"));
    let counts = deepref_app::project_counts::article_counts(
        &state.pool,
        &[(workspace, project)],
        state.project_count_cache.as_deref(),
        primary_counts(&headers),
    )
    .await?;
    let count = counts
        .get(&project)
        .ok_or_else(|| ApiError::NotFound("project not found".into()))?;
    Ok(Json(project_with_count(
        row,
        count.count,
        count.observed_at,
    )))
}

#[utoipa::path(
    patch,
    path = "/projects/{project_id}",
    operation_id = "updateProject",
    tag = "projects",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    request_body = CreateProject,
    responses(
        (status = 200, description = "Updated project", body = ProjectDto),
        (status = 400, description = "Invalid project", body = ErrorResponse),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn update_project(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Json(input): Json<CreateProject>,
) -> Result<Json<ProjectDto>, ApiError> {
    let input = input.validate()?;
    let row = sqlx::query(
        r#"
        UPDATE projects SET name = $2, description = $3,
          default_max_depth = COALESCE($4, default_max_depth), updated_at = now()
        WHERE id = $1
        RETURNING id, name, description, default_max_depth, created_at, updated_at,
          (SELECT count(*) FROM project_reports pr WHERE pr.project_id = projects.id) AS article_count,
          statement_timestamp() AS article_count_as_of
        "#,
    )
    .bind(project_id)
    .bind(input.name)
    .bind(input.description)
    .bind(input.default_max_depth)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(project_from_row(row)))
}

#[utoipa::path(
    delete,
    path = "/projects/{project_id}",
    operation_id = "deleteProject",
    tag = "projects",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    responses(
        (status = 204, description = "Project and everything that belongs to it deleted"),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn delete_project(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    session: Option<CurrentSession>,
) -> Result<StatusCode, ApiError> {
    let actor = session
        .as_ref()
        .and_then(|CurrentSession(context)| context.principal.user_id());
    let mut tx = state.pool.begin().await?;
    // The audit row names the workspace the project leaves (the trigger
    // guarantees one; the migration workspace is its default).
    let workspace: Option<Uuid> =
        sqlx::query_scalar("SELECT workspace_id FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(workspace_id) = workspace else {
        return Err(ApiError::NotFound("project not found".into()));
    };
    if !deepref_app::delete_project_in_transaction(&mut tx, project_id).await? {
        return Err(ApiError::NotFound("project not found".into()));
    }
    let workspace = WorkspaceId::new(workspace_id);
    deepref_app::record_project_deleted(
        &mut tx,
        actor,
        workspace,
        ProjectId::new(project_id),
        AuditRequest {
            request_id: crate::error::correlation_id().as_deref(),
            session_ref: None,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

fn primary_counts(headers: &HeaderMap) -> bool {
    headers
        .get_all(axum::http::header::CACHE_CONTROL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|token| {
            matches!(
                token.trim().to_ascii_lowercase().as_str(),
                "no-cache" | "no-store" | "max-age=0"
            )
        })
}

fn project_from_row(row: sqlx::postgres::PgRow) -> ProjectDto {
    let count = row.get("article_count");
    let observed_at = row.get("article_count_as_of");
    project_with_count(row, count, observed_at)
}

fn project_with_count(
    row: sqlx::postgres::PgRow,
    count: i64,
    observed_at: DateTime<Utc>,
) -> ProjectDto {
    ProjectDto {
        id: row.get("id"),
        name: row.get("name"),
        description: row.get("description"),
        default_max_depth: row.get("default_max_depth"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        article_count: count,
        article_count_as_of: Some(observed_at),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_blank_project_names() {
        assert!(
            CreateProject {
                name: " ".to_owned(),
                description: None,
                default_max_depth: None,
                workspace_id: None,
            }
            .validate()
            .is_err()
        );
    }
}
