use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use chrono::{DateTime, Utc};
use deepref_core::{IngestionStatus, normalize_doi};
use deepref_events::{EventEnvelope, SUBJECT_WORK_FETCH_REQUESTED, WorkFetchRequested};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    error::{ApiError, ErrorResponse},
    outbox,
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
        if self.seed_dois.is_empty() {
            return Err(ApiError::BadRequest(
                "seed_dois must not be empty".to_owned(),
            ));
        }
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
        let seed_dois = self
            .seed_dois
            .iter()
            .map(|doi| normalize_doi(doi))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(ValidatedIngestion {
            project_id: self.project_id,
            seed_dois,
            max_depth: self.max_depth,
            metadata_provider,
            citation_provider,
        })
    }
}

#[derive(Debug, Serialize, ToSchema)]
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

#[derive(Debug, Serialize, ToSchema)]
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
    .bind(input.metadata_provider)
    .bind(input.citation_provider)
    .fetch_one(&mut *tx)
    .await?;

    for doi in &input.seed_dois {
        sqlx::query(
            r#"
            INSERT INTO ingestion_items (ingestion_id, project_id, canonical_doi, depth, parent_doi, status)
            VALUES ($1, $2, $3, 0, NULL, 'queued')
            ON CONFLICT (ingestion_id, canonical_doi) DO NOTHING
            "#,
        )
        .bind(ingestion_id)
        .bind(input.project_id)
        .bind(doi)
        .execute(&mut *tx)
        .await?;
    }
    for doi in input.seed_dois {
        let payload = WorkFetchRequested {
            project_id: input.project_id,
            ingestion_id,
            doi: doi.clone(),
            depth: 0,
            max_depth,
            parent_doi: None,
        };
        let event = EventEnvelope::new(
            SUBJECT_WORK_FETCH_REQUESTED,
            "deepref.api",
            format!("doi:{doi}"),
            ingestion_id,
            None,
            payload,
        );
        outbox::enqueue(&mut tx, event.id, SUBJECT_WORK_FETCH_REQUESTED, &event).await?;
    }
    tx.commit().await?;

    Ok((StatusCode::CREATED, Json(ingestion_from_row(row))))
}

#[utoipa::path(
    get,
    path = "/ingestions",
    operation_id = "listIngestions",
    tag = "ingestions",
    responses(
        (status = 200, description = "Ingestions ordered newest first", body = [IngestionDto]),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_ingestions(
    State(state): State<AppState>,
) -> Result<Json<Vec<IngestionDto>>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT id, project_id, status, max_depth, seed_count, queued_count, fetched_count,
               failed_count, created_at, started_at, completed_at
        FROM ingestions ORDER BY created_at DESC
        "#,
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows.into_iter().map(ingestion_from_row).collect()))
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
        (status = 200, description = "Ingestion items", body = [IngestionItemDto]),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_ingestion_items(
    State(state): State<AppState>,
    Path(ingestion_id): Path<Uuid>,
) -> Result<Json<Vec<IngestionItemDto>>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT canonical_doi, depth, parent_doi, status, attempts, last_error, queued_at, fetched_at
        FROM ingestion_items WHERE ingestion_id = $1 ORDER BY queued_at DESC
        "#,
    )
    .bind(ingestion_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter().map(ingestion_item_from_row).collect(),
    ))
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
}
