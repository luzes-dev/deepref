use std::collections::HashSet;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Utc};
use deepref_application::{CsvColumnMapping, ImportParser};
use deepref_core::normalize_doi;
use deepref_domain::{ImportFormat, ProjectId};
use deepref_events::{EntityType, EventEnvelope, SUBJECT_WORK_FETCH_REQUESTED, WorkFetchRequested};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::Row;
use utoipa::ToSchema;
use uuid::Uuid;

use super::pagination::{PaginatedResponse, PaginationParams, page};
use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};

pub(crate) const MAX_IMPORT_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_SEED_DOIS: usize = 1_000;
pub(crate) const MAX_REQUEST_BODY_BYTES: usize = MAX_IMPORT_BYTES * 6 + 64 * 1024;

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateAcquisition {
    pub seed_dois: Vec<String>,
    pub max_depth: Option<i32>,
    pub metadata_provider: Option<String>,
    pub citation_provider: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ImportRecords {
    /// One of doi, ris, bibtex, nbib, or csv.
    pub format: String,
    pub content: String,
    #[schema(value_type = Option<Object>)]
    pub csv_mapping: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct AcquisitionDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub source: String,
    pub strategy: String,
    pub format: Option<String>,
    pub status: String,
    pub seed_count: i32,
    pub queued_count: i32,
    pub fetched_count: i32,
    pub failed_count: i32,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/acquisitions",
    operation_id = "listAcquisitions",
    tag = "acquisitions",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("cursor" = Option<String>, Query, description = "Opaque pagination cursor"),
        ("limit" = Option<i64>, Query, description = "Page size")
    ),
    responses(
        (status = 200, description = "Acquisition runs ordered newest first", body = PaginatedResponse<AcquisitionDto>),
        (status = 400, description = "Invalid pagination cursor", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_acquisitions(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Query(pagination): Query<PaginationParams>,
) -> Result<Json<PaginatedResponse<AcquisitionDto>>, ApiError> {
    let limit = pagination.limit()?;
    let cursor: Option<(DateTime<Utc>, Uuid)> = pagination.decode()?;
    let rows = sqlx::query(
        "SELECT id,project_id,source,strategy,format,status,seed_count,queued_count,fetched_count,
                failed_count,created_at,started_at,completed_at
         FROM acquisition_runs
         WHERE project_id=$1 AND ($2::timestamptz IS NULL OR (created_at,id)<($2,$3))
         ORDER BY created_at DESC,id DESC LIMIT $4",
    )
    .bind(project_id)
    .bind(cursor.as_ref().map(|value| value.0))
    .bind(cursor.as_ref().map(|value| value.1))
    .bind(limit + 1)
    .fetch_all(&state.pool)
    .await?;
    let items = rows.into_iter().map(acquisition_from_row).collect();
    Ok(Json(page(items, limit as usize, |item| {
        (item.created_at, item.id)
    })?))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/acquisitions",
    operation_id = "createAcquisition",
    tag = "acquisitions",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("Idempotency-Key" = Option<String>, Header, description = "Stable key for replay-safe acquisition creation")
    ),
    request_body = CreateAcquisition,
    responses(
        (status = 200, description = "Existing idempotent acquisition", body = AcquisitionDto),
        (status = 201, description = "Acquisition created", body = AcquisitionDto),
        (status = 400, description = "Invalid acquisition", body = ErrorResponse),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 409, description = "Idempotency conflict", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn create_acquisition(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<CreateAcquisition>,
) -> Result<(StatusCode, Json<AcquisitionDto>), ApiError> {
    let seed_dois = validate_seeds(input.seed_dois)?;
    let requested_max_depth = input.max_depth;
    if requested_max_depth.is_some_and(|value| value < 0) {
        return Err(ApiError::BadRequest("max_depth must be >= 0".to_owned()));
    }
    let metadata_provider = input
        .metadata_provider
        .unwrap_or_else(|| "crossref".to_owned());
    let citation_provider = input
        .citation_provider
        .unwrap_or_else(|| "crossref".to_owned());
    if metadata_provider != "crossref" || citation_provider != "crossref" {
        return Err(ApiError::BadRequest(
            "only the crossref provider is supported for DOI acquisitions".to_owned(),
        ));
    }
    let idempotency_key = idempotency_key(&headers)?;
    let mut tx = state.pool.begin().await?;
    let project_default_depth: i32 =
        sqlx::query_scalar("SELECT default_max_depth FROM projects WHERE id=$1")
            .bind(project_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| ApiError::NotFound("project not found".to_owned()))?;
    let max_depth = requested_max_depth.unwrap_or(project_default_depth);
    let config = json!({
        "seed_dois": seed_dois,
        "max_depth": max_depth,
        "metadata_provider": metadata_provider,
        "citation_provider": citation_provider,
    });
    let run_id = idempotency_key.as_deref().map_or_else(Uuid::new_v4, |key| {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("deepref:acquisition:{project_id}:{key}").as_bytes(),
        )
    });
    let seed_count = i32::try_from(seed_dois.len()).unwrap_or(i32::MAX);
    let ingestion_inserted = if idempotency_key.is_some() {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO ingestions
             (id,project_id,status,max_depth,seed_count,queued_count,metadata_provider,citation_provider)
             VALUES ($1,$2,'queued',$3,$4,$4,$5,$6)
             ON CONFLICT (id) DO NOTHING RETURNING id",
        )
        .bind(run_id)
        .bind(project_id)
        .bind(max_depth)
        .bind(seed_count)
        .bind(&metadata_provider)
        .bind(&citation_provider)
        .fetch_optional(&mut *tx)
        .await?
        .is_some()
    } else {
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO ingestions
             (id,project_id,status,max_depth,seed_count,queued_count,metadata_provider,citation_provider)
             VALUES ($1,$2,'queued',$3,$4,$4,$5,$6) RETURNING id",
        )
        .bind(run_id)
        .bind(project_id)
        .bind(max_depth)
        .bind(seed_count)
        .bind(&metadata_provider)
        .bind(&citation_provider)
        .fetch_one(&mut *tx)
        .await?;
        true
    };
    let inserted_run_id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO acquisition_runs
         (id,project_id,legacy_ingestion_id,source,strategy,format,idempotency_key,config,metadata,status,
          max_depth,seed_count,queued_count,fetched_count,failed_count,metadata_provider,citation_provider,created_at)
         VALUES ($1,$2,$3,'crossref','citation_traversal',NULL,$4,$5,$6,'queued',$7,$8,$8,0,0,$9,$10,now())
         ON CONFLICT (project_id,idempotency_key) WHERE idempotency_key IS NOT NULL DO NOTHING
         RETURNING id",
    )
    .bind(run_id)
    .bind(project_id)
    .bind(run_id)
    .bind(idempotency_key.as_deref())
    .bind(&config)
    .bind(json!({ "seed_dois": seed_count }))
    .bind(max_depth)
    .bind(seed_count)
    .bind(&metadata_provider)
    .bind(&citation_provider)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(run_id) = inserted_run_id else {
        if ingestion_inserted {
            sqlx::query("DELETE FROM ingestions WHERE id=$1")
                .bind(run_id)
                .execute(&mut *tx)
                .await?;
        }
        let row = sqlx::query(
            "SELECT id,config FROM acquisition_runs WHERE project_id=$1 AND idempotency_key=$2 FOR UPDATE",
        )
        .bind(project_id)
        .bind(idempotency_key.as_deref())
        .fetch_one(&mut *tx)
        .await?;
        let existing_id: Uuid = row.get("id");
        let existing_config: serde_json::Value = row.get("config");
        if existing_config != config {
            return Err(ApiError::Conflict {
                code: "IDEMPOTENCY_KEY_REUSED".to_owned(),
                message: "Idempotency-Key was already used with different acquisition input"
                    .to_owned(),
                details: json!({ "acquisition_id": existing_id }),
            });
        }
        let run = sqlx::query(acquisition_select())
            .bind(existing_id)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok((StatusCode::OK, Json(acquisition_from_row(run))));
    };

    enqueue_seed_jobs(&mut tx, project_id, run_id, max_depth, &seed_dois).await?;
    let run = sqlx::query(acquisition_select())
        .bind(run_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(acquisition_from_row(run))))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/imports",
    operation_id = "importProjectRecords",
    tag = "acquisitions",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("Idempotency-Key" = Option<String>, Header, description = "Stable key for replay-safe imports")
    ),
    request_body = ImportRecords,
    responses(
        (status = 201, description = "Import created", body = AcquisitionDto),
        (status = 200, description = "Existing idempotent import", body = AcquisitionDto),
        (status = 400, description = "Invalid import", body = ErrorResponse),
        (status = 404, description = "Project not found", body = ErrorResponse),
        (status = 409, description = "Idempotency conflict", body = ErrorResponse),
        (status = 413, description = "Import exceeds the request size limit", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn import_project_records(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<ImportRecords>,
) -> Result<(StatusCode, Json<AcquisitionDto>), ApiError> {
    if input.content.len() > MAX_IMPORT_BYTES {
        return Err(ApiError::PayloadTooLarge(format!(
            "import content exceeds the {} byte limit",
            MAX_IMPORT_BYTES
        )));
    }
    if input.content.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "import content must not be empty".to_owned(),
        ));
    }
    let format = parse_format(&input.format)?;
    let csv_mapping = match input.csv_mapping {
        Some(mapping) => Some(serde_json::from_value::<CsvColumnMapping>(mapping)?),
        None => None,
    };
    if matches!(format, ImportFormat::Csv) && csv_mapping.is_none() {
        return Err(ApiError::BadRequest(
            "CSV imports require csv_mapping".to_owned(),
        ));
    }
    let parser = deepref_providers::ImportParserAdapter::new(format, csv_mapping.clone());
    let records = parser
        .parse(input.content.as_bytes())
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let idempotency_key = idempotency_key(&headers)?;
    let digest = Sha256::digest(input.content.as_bytes());
    let content_sha256 = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let config = json!({
        "format": format.as_str(),
        "content_sha256": content_sha256,
        "csv_mapping": csv_mapping,
    });
    let result = deepref_postgres::persist_import(
        &state.pool,
        &deepref_postgres::ImportPersistRequest {
            project_id,
            source: format!("import:{}", format.as_str()),
            strategy: "file_import".to_owned(),
            format,
            idempotency_key,
            config,
            metadata: json!({ "content_bytes": input.content.len() }),
        },
        &records,
    )
    .await
    .map_err(map_acquisition_error)?;
    let run = sqlx::query(acquisition_select())
        .bind(result.run_id)
        .fetch_one(&state.pool)
        .await?;
    Ok((
        if result.created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        Json(acquisition_from_row(run)),
    ))
}

pub(crate) async fn enqueue_seed_jobs(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    project_id: Uuid,
    ingestion_id: Uuid,
    max_depth: i32,
    seed_dois: &[String],
) -> Result<(), ApiError> {
    for doi in seed_dois {
        let revision: i64 = sqlx::query_scalar("SELECT nextval('graph_domain_revision_seq')")
            .fetch_one(&mut **tx)
            .await?;
        let event = EventEnvelope::v1(
            SUBJECT_WORK_FETCH_REQUESTED,
            "deepref.api",
            EntityType::Work,
            format!("{ingestion_id}|{doi}"),
            revision,
            ingestion_id,
            None,
            WorkFetchRequested {
                project_id,
                ingestion_id,
                doi: doi.clone(),
                depth: 0,
                max_depth,
                parent_doi: None,
            },
        );
        sqlx::query(
            "INSERT INTO domain_events (event_id,schema_version,event_type,entity_type,entity_key,revision,payload,correlation_id,causation_id,created_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT (event_id) DO NOTHING",
        )
        .bind(event.event_id)
        .bind(event.schema_version as i16)
        .bind(&event.event_type)
        .bind(event.entity_type.as_str())
        .bind(&event.entity_key)
        .bind(event.revision)
        .bind(serde_json::to_value(&event.payload)?)
        .bind(event.correlation_id)
        .bind(event.causation_id)
        .bind(event.occurred_at)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "INSERT INTO ingestion_items (ingestion_id,project_id,canonical_doi,depth,parent_doi,status,work_event_id)
             VALUES ($1,$2,$3,0,NULL,'queued',$4) ON CONFLICT (ingestion_id,canonical_doi) DO NOTHING",
        )
        .bind(ingestion_id)
        .bind(project_id)
        .bind(doi)
        .bind(event.event_id)
        .execute(&mut **tx)
        .await?;
        deepref_postgres::enqueue_job(
            tx,
            &deepref_postgres::job(
                event.event_id,
                ProjectId::new(project_id),
                "work_fetch_requested",
                serde_json::to_value(&event)?,
                format!("work_fetch:{}", event.event_id),
            ),
        )
        .await?;
    }
    Ok(())
}

fn acquisition_select() -> &'static str {
    "SELECT id,project_id,source,strategy,format,status,seed_count,queued_count,fetched_count,
            failed_count,created_at,started_at,completed_at FROM acquisition_runs WHERE id=$1"
}

pub(crate) fn validate_seeds(seed_dois: Vec<String>) -> Result<Vec<String>, ApiError> {
    if seed_dois.is_empty() {
        return Err(ApiError::BadRequest(
            "seed_dois must not be empty".to_owned(),
        ));
    }
    if seed_dois.len() > MAX_SEED_DOIS {
        return Err(ApiError::BadRequest(format!(
            "seed_dois must contain at most {MAX_SEED_DOIS} items"
        )));
    }
    let mut seen = HashSet::with_capacity(seed_dois.len());
    let normalized = seed_dois
        .into_iter()
        .map(|doi| normalize_doi(&doi).map_err(ApiError::from))
        .try_fold(Vec::new(), |mut normalized, doi| {
            let doi = doi?;
            if seen.insert(doi.clone()) {
                normalized.push(doi);
            }
            Ok::<_, ApiError>(normalized)
        })?;
    if normalized.len() > MAX_SEED_DOIS {
        return Err(ApiError::BadRequest(format!(
            "seed_dois must contain at most {MAX_SEED_DOIS} unique items"
        )));
    }
    Ok(normalized)
}

fn parse_format(value: &str) -> Result<ImportFormat, ApiError> {
    match value.trim().to_lowercase().as_str() {
        "doi" | "dois" | "doi_list" => Ok(ImportFormat::Doi),
        "ris" => Ok(ImportFormat::Ris),
        "bibtex" | "biblatex" | "bib" => Ok(ImportFormat::Bibtex),
        "nbib" | "pubmed" => Ok(ImportFormat::Nbib),
        "csv" => Ok(ImportFormat::Csv),
        other => Err(ApiError::BadRequest(format!(
            "unsupported import format: {other}"
        ))),
    }
}

fn idempotency_key(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    headers
        .get("idempotency-key")
        .map(|value| {
            let key = value
                .to_str()
                .map_err(|_| {
                    ApiError::BadRequest("Idempotency-Key must be valid ASCII".to_owned())
                })?
                .trim();
            if key.is_empty() || key.len() > 200 {
                return Err(ApiError::BadRequest(
                    "Idempotency-Key must contain 1 through 200 characters".to_owned(),
                ));
            }
            Ok(key.to_owned())
        })
        .transpose()
}

fn map_acquisition_error(error: deepref_postgres::AcquisitionError) -> ApiError {
    match error {
        deepref_postgres::AcquisitionError::ProjectNotFound => {
            ApiError::NotFound("project not found".to_owned())
        }
        deepref_postgres::AcquisitionError::IdempotencyConflict { run_id } => ApiError::Conflict {
            code: "IDEMPOTENCY_KEY_REUSED".to_owned(),
            message: "Idempotency-Key was already used with different import content".to_owned(),
            details: json!({ "acquisition_id": run_id }),
        },
        deepref_postgres::AcquisitionError::Database(error) => ApiError::Database(error),
        deepref_postgres::AcquisitionError::Serialization(error) => {
            ApiError::Internal(error.into())
        }
    }
}

fn acquisition_from_row(row: sqlx::postgres::PgRow) -> AcquisitionDto {
    AcquisitionDto {
        id: row.get("id"),
        project_id: row.get("project_id"),
        source: row.get("source"),
        strategy: row.get("strategy"),
        format: row.get("format"),
        status: row.get("status"),
        seed_count: row.get("seed_count"),
        queued_count: row.get("queued_count"),
        fetched_count: row.get("fetched_count"),
        failed_count: row.get("failed_count"),
        created_at: row.get("created_at"),
        started_at: row.get("started_at"),
        completed_at: row.get("completed_at"),
    }
}
