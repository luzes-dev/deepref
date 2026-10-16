use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
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
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateProject {
    name: String,
    description: Option<String>,
    default_max_depth: Option<i32>,
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
    responses(
        (status = 200, description = "Projects ordered by most recently updated", body = PaginatedResponse<ProjectDto>),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_projects(
    State(state): State<AppState>,
    Query(pagination): Query<PaginationParams>,
) -> Result<Json<PaginatedResponse<ProjectDto>>, ApiError> {
    let limit = pagination.limit()?;
    let cursor: Option<(DateTime<Utc>, Uuid)> = pagination.decode()?;
    let rows = sqlx::query(
        "SELECT id,name,description,default_max_depth,created_at,updated_at,\
         (SELECT count(*) FROM project_reports pr WHERE pr.project_id = projects.id) AS article_count \
         FROM projects \
         WHERE ($1::timestamptz IS NULL OR (updated_at,id)<($1,$2)) \
         ORDER BY updated_at DESC,id DESC LIMIT $3",
    )
    .bind(cursor.as_ref().map(|value| value.0))
    .bind(cursor.as_ref().map(|value| value.1))
    .bind(limit + 1)
    .fetch_all(&state.pool)
    .await?;
    let projects = rows.into_iter().map(project_from_row).collect();
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
    Json(input): Json<CreateProject>,
) -> Result<(StatusCode, Json<ProjectDto>), ApiError> {
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
    let row = sqlx::query(
        r#"
        INSERT INTO projects (id, name, description, default_max_depth)
        VALUES ($1, $2, $3, $4)
        RETURNING id, name, description, default_max_depth, created_at, updated_at,
          0::bigint AS article_count
        "#,
    )
    .bind(id)
    .bind(input.name)
    .bind(input.description)
    .bind(default_max_depth)
    .fetch_one(&state.pool)
    .await?;
    Ok((StatusCode::CREATED, Json(project_from_row(row))))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}",
    operation_id = "getProject",
    tag = "projects",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    responses(
        (status = 200, description = "Project", body = ProjectDto),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_project(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<ProjectDto>, ApiError> {
    let row = sqlx::query(
        "SELECT id, name, description, default_max_depth, created_at, updated_at,\
         (SELECT count(*) FROM project_reports pr WHERE pr.project_id = projects.id) AS article_count \
         FROM projects WHERE id = $1",
    )
    .bind(project_id)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(project_from_row(row)))
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
          (SELECT count(*) FROM project_reports pr WHERE pr.project_id = projects.id) AS article_count
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
) -> Result<StatusCode, ApiError> {
    let mut tx = state.pool.begin().await?;
    if !deepref_app::delete_project_in_transaction(&mut tx, project_id).await? {
        return Err(ApiError::NotFound("project not found".into()));
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

fn project_from_row(row: sqlx::postgres::PgRow) -> ProjectDto {
    ProjectDto {
        id: row.get("id"),
        name: row.get("name"),
        description: row.get("description"),
        default_max_depth: row.get("default_max_depth"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        article_count: row.get("article_count"),
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
            }
            .validate()
            .is_err()
        );
    }
}
