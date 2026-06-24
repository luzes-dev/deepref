use axum::{
    Json,
    extract::{Path, State},
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

use super::settings;

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ProjectDto {
    id: Uuid,
    name: String,
    description: Option<String>,
    default_max_depth: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
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
        if self.default_max_depth.is_some_and(|value| value < 0) {
            return Err(ApiError::BadRequest(
                "default_max_depth must be >= 0".to_owned(),
            ));
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
        (status = 200, description = "Projects ordered by most recently updated", body = [ProjectDto]),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_projects(
    State(state): State<AppState>,
) -> Result<Json<Vec<ProjectDto>>, ApiError> {
    let rows = sqlx::query(
        "SELECT id, name, description, default_max_depth, created_at, updated_at FROM projects ORDER BY updated_at DESC",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows.into_iter().map(project_from_row).collect()))
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
        RETURNING id, name, description, default_max_depth, created_at, updated_at
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
        "SELECT id, name, description, default_max_depth, created_at, updated_at FROM projects WHERE id = $1",
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
        RETURNING id, name, description, default_max_depth, created_at, updated_at
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
        (status = 204, description = "Project deleted"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn delete_project(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    sqlx::query("DELETE FROM projects WHERE id = $1")
        .bind(project_id)
        .execute(&state.pool)
        .await?;
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
