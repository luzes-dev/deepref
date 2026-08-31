use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use chrono::{DateTime, Utc};
use deepref_core::IngestionStatus;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use utoipa::ToSchema;
use uuid::Uuid;

use super::pagination::{PaginatedResponse, PaginationParams, page};
use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateIngestion {
    project_id: Uuid,
    seed_dois: Vec<String>,
    max_depth: Option<i32>,
    metadata_provider: Option<String>,
    citation_provider: Option<String>,
}

struct ValidatedIngestion {
    project_id: Uuid,
    seed_dois: Vec<String>,
    max_depth: Option<i32>,
    metadata_provider: String,
    citation_provider: String,
}

impl CreateIngestion {
    fn validate(self) -> Result<ValidatedIngestion, ApiError> {
        if self.max_depth.is_some_and(|value| value < 0) {
            return Err(ApiError::BadRequest("max_depth must be >= 0".to_owned()));
        }
        let metadata_provider = self
            .metadata_provider
            .unwrap_or_else(|| "crossref".to_owned());
        let citation_provider = self
            .citation_provider
            .unwrap_or_else(|| "crossref".to_owned());
        if metadata_provider != "crossref" || citation_provider != "crossref" {
            return Err(ApiError::BadRequest(
                "only the crossref provider is supported".to_owned(),
            ));
        }
        let seed_dois = super::acquisitions::validate_seeds(self.seed_dois)?;

        Ok(ValidatedIngestion {
            project_id: self.project_id,
            seed_dois,
            max_depth: self.max_depth,
            metadata_provider,
            citation_provider,
        })
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct IngestionDto {
    pub(crate) id: Uuid,
    pub(crate) project_id: Uuid,
    pub(crate) status: String,
    pub(crate) max_depth: i32,
    pub(crate) seed_count: i32,
    pub(crate) queued_count: i32,
    pub(crate) fetched_count: i32,
    pub(crate) failed_count: i32,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) started_at: Option<DateTime<Utc>>,
    pub(crate) completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct IngestionItemDto {
    doi: String,
    depth: i32,
    parent_doi: Option<String>,
    status: String,
    attempts: i32,
    last_error: Option<String>,
    queued_at: DateTime<Utc>,
    fetched_at: Option<DateTime<Utc>>,
}

#[utoipa::path(
    post,
    path = "/ingestions",
    operation_id = "createIngestion",
    tag = "ingestions",
    request_body = CreateIngestion,
    responses(
        (status = 201, description = "Ingestion created", body = IngestionDto),
        (status = 400, description = "Invalid ingestion", body = ErrorResponse),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn create_ingestion(
    State(state): State<AppState>,
    Json(input): Json<CreateIngestion>,
) -> Result<(StatusCode, Json<IngestionDto>), ApiError> {
    let input = input.validate()?;
    let crossref_mailto: String =
        sqlx::query_scalar("SELECT crossref_mailto FROM settings WHERE id=1")
            .fetch_optional(&state.pool)
            .await?
            .unwrap_or_default();
    if !valid_email(&crossref_mailto) {
        return Err(ApiError::Configuration(
            "a valid Crossref mail address must be configured before creating an ingestion".into(),
        ));
    }
    let ingestion_id = Uuid::new_v4();
    let max_depth = match input.max_depth {
        Some(value) => value,
        None => {
            let row = sqlx::query("SELECT default_max_depth FROM projects WHERE id = $1")
                .bind(input.project_id)
                .fetch_one(&state.pool)
                .await?;
            row.get("default_max_depth")
        }
    };

    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        r#"
        INSERT INTO ingestions
          (id, project_id, status, max_depth, seed_count, queued_count, metadata_provider, citation_provider)
        VALUES ($1, $2, 'queued', $3, $4, $4, $5, $6)
        RETURNING id, project_id, status, max_depth, seed_count, queued_count, fetched_count,
                  failed_count, created_at, started_at, completed_at
        "#,
    )
    .bind(ingestion_id)
    .bind(input.project_id)
    .bind(max_depth)
    .bind(input.seed_dois.len() as i32)
    .bind(&input.metadata_provider)
    .bind(&input.citation_provider)
    .fetch_one(&mut *tx)
    .await?;
    deepref_postgres::ensure_legacy_acquisition_run(
        &mut tx,
        ingestion_id,
        input.project_id,
        max_depth,
        input.seed_dois.len() as i32,
        &input.metadata_provider,
        &input.citation_provider,
    )
    .await?;

    super::acquisitions::enqueue_seed_jobs(
        &mut tx,
        input.project_id,
        ingestion_id,
        max_depth,
        &input.seed_dois,
    )
    .await?;
    tx.commit().await?;

    Ok((StatusCode::CREATED, Json(ingestion_from_row(row))))
}

#[utoipa::path(
    get,
    path = "/ingestions",
    operation_id = "listIngestions",
    tag = "ingestions",
    responses(
        (status = 200, description = "Ingestions ordered newest first", body = PaginatedResponse<IngestionDto>),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_ingestions(
    State(state): State<AppState>,
    Query(pagination): Query<PaginationParams>,
) -> Result<Json<PaginatedResponse<IngestionDto>>, ApiError> {
    let limit = pagination.limit()?;
    let cursor: Option<(DateTime<Utc>, Uuid)> = pagination.decode()?;
    let rows = sqlx::query(
        r#"
        SELECT id, project_id, status, max_depth, seed_count, queued_count, fetched_count,
               failed_count, created_at, started_at, completed_at
        FROM ingestions WHERE ($1::timestamptz IS NULL OR (created_at,id)<($1,$2))
        ORDER BY created_at DESC,id DESC LIMIT $3
        "#,
    )
    .bind(cursor.as_ref().map(|value| value.0))
    .bind(cursor.as_ref().map(|value| value.1))
    .bind(limit + 1)
    .fetch_all(&state.pool)
    .await?;
    let items = rows.into_iter().map(ingestion_from_row).collect();
    Ok(Json(page(items, limit as usize, |item| {
        (item.created_at, item.id)
    })?))
}

#[utoipa::path(
    get,
    path = "/ingestions/{ingestion_id}",
    operation_id = "getIngestion",
    tag = "ingestions",
    params(("ingestion_id" = Uuid, Path, description = "Ingestion identifier")),
    responses(
        (status = 200, description = "Ingestion", body = IngestionDto),
        (status = 404, description = "Ingestion not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_ingestion(
    State(state): State<AppState>,
    Path(ingestion_id): Path<Uuid>,
) -> Result<Json<IngestionDto>, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT id, project_id, status, max_depth, seed_count, queued_count, fetched_count,
               failed_count, created_at, started_at, completed_at
        FROM ingestions WHERE id = $1
        "#,
    )
    .bind(ingestion_id)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(ingestion_from_row(row)))
}

#[utoipa::path(
    post,
    path = "/ingestions/{ingestion_id}/cancel",
    operation_id = "cancelIngestion",
    tag = "ingestions",
    params(("ingestion_id" = Uuid, Path, description = "Ingestion identifier")),
    responses(
        (status = 202, description = "Cancellation accepted"),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn cancel_ingestion(
    State(state): State<AppState>,
    Path(ingestion_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    sqlx::query("UPDATE ingestions SET status = $2, completed_at = now() WHERE id = $1")
        .bind(ingestion_id)
        .bind(IngestionStatus::Cancelled.as_str())
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::ACCEPTED)
}

#[utoipa::path(
    get,
    path = "/ingestions/{ingestion_id}/items",
    operation_id = "listIngestionItems",
    tag = "ingestions",
    params(("ingestion_id" = Uuid, Path, description = "Ingestion identifier")),
    responses(
        (status = 200, description = "Ingestion items", body = PaginatedResponse<IngestionItemDto>),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_ingestion_items(
    State(state): State<AppState>,
    Path(ingestion_id): Path<Uuid>,
    Query(pagination): Query<PaginationParams>,
) -> Result<Json<PaginatedResponse<IngestionItemDto>>, ApiError> {
    let limit = pagination.limit()?;
    let cursor: Option<(DateTime<Utc>, String)> = pagination.decode()?;
    let rows = sqlx::query(
        r#"
        SELECT canonical_doi, depth, parent_doi, status, attempts, last_error, queued_at, fetched_at
        FROM ingestion_items WHERE ingestion_id = $1
          AND ($2::timestamptz IS NULL OR (queued_at,canonical_doi)<($2,$3))
        ORDER BY queued_at DESC,canonical_doi DESC LIMIT $4
        "#,
    )
    .bind(ingestion_id)
    .bind(cursor.as_ref().map(|value| value.0))
    .bind(cursor.as_ref().map(|value| value.1.clone()))
    .bind(limit + 1)
    .fetch_all(&state.pool)
    .await?;
    let items = rows.into_iter().map(ingestion_item_from_row).collect();
    Ok(Json(page(items, limit as usize, |item| {
        (item.queued_at, item.doi.clone())
    })?))
}

fn valid_email(value: &str) -> bool {
    let value = value.trim();
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

pub(crate) fn ingestion_from_row(row: sqlx::postgres::PgRow) -> IngestionDto {
    IngestionDto {
        id: row.get("id"),
        project_id: row.get("project_id"),
        status: row.get("status"),
        max_depth: row.get("max_depth"),
        seed_count: row.get("seed_count"),
        queued_count: row.get("queued_count"),
        fetched_count: row.get("fetched_count"),
        failed_count: row.get("failed_count"),
        created_at: row.get("created_at"),
        started_at: row.get("started_at"),
        completed_at: row.get("completed_at"),
    }
}

pub(crate) fn ingestion_item_from_row(row: sqlx::postgres::PgRow) -> IngestionItemDto {
    IngestionItemDto {
        doi: row.get("canonical_doi"),
        depth: row.get("depth"),
        parent_doi: row.get("parent_doi"),
        status: row.get("status"),
        attempts: row.get("attempts"),
        last_error: row.get("last_error"),
        queued_at: row.get("queued_at"),
        fetched_at: row.get("fetched_at"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_seed_dois() {
        assert!(
            CreateIngestion {
                project_id: Uuid::new_v4(),
                seed_dois: vec![],
                max_depth: None,
                metadata_provider: None,
                citation_provider: None,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn rejects_unknown_providers() {
        assert!(
            CreateIngestion {
                project_id: Uuid::new_v4(),
                seed_dois: vec!["10.1/x".to_owned()],
                max_depth: None,
                metadata_provider: Some("other".to_owned()),
                citation_provider: None,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn normalizes_and_deduplicates_seed_dois_before_persistence() {
        let validated = CreateIngestion {
            project_id: Uuid::new_v4(),
            seed_dois: vec![
                "https://doi.org/10.5555/SEED.".to_owned(),
                "doi:10.5555/seed".to_owned(),
            ],
            max_depth: None,
            metadata_provider: None,
            citation_provider: None,
        }
        .validate()
        .unwrap();
        assert_eq!(validated.seed_dois, vec!["10.5555/seed"]);
    }

    #[test]
    fn rejects_seed_lists_over_the_explicit_bound() {
        let seed_dois = (0..=crate::routes::acquisitions::MAX_SEED_DOIS)
            .map(|index| format!("10.5555/seed-{index}"))
            .collect();
        assert!(
            CreateIngestion {
                project_id: Uuid::new_v4(),
                seed_dois,
                max_depth: None,
                metadata_provider: None,
                citation_provider: None,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn validates_crossref_email_shape() {
        assert!(valid_email("ops@example.com"));
        assert!(!valid_email("blank"));
    }
}
