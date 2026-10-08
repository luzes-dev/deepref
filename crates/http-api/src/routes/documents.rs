use axum::{
    Json,
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use deepref_domain::ProjectId;
use futures::{StreamExt, TryStreamExt, stream};
use serde::{Deserialize, Serialize};
use sqlx::Error as SqlxError;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    error::{ApiError, ErrorResponse},
    state::AppState,
};

pub(crate) const MAX_DOCUMENT_LIST_LIMIT: i64 = 100;
pub(crate) const MAX_FILENAME_BYTES: usize = 255;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct DocumentDto {
    pub id: Uuid,
    pub report_id: Uuid,
    pub original_filename: Option<String>,
    pub source: String,
    pub status: String,
    /// The address the PDF is retrieved from, for external documents.
    pub external_url: Option<String>,
    pub mime_type: String,
    pub byte_size: i64,
    pub content_hash: Option<String>,
    pub parser_version: Option<String>,
    pub parser_error: Option<String>,
    pub ocr_required: bool,
    /// Whether the PDF's title and DOI agree with the report. `None` until the PDF is parsed.
    pub identity: Option<DocumentIdentityDto>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct DocumentIdentityDto {
    /// `match`, `mismatch` or `unknown`.
    pub verdict: String,
    /// The title the PDF states about itself.
    pub detected_title: Option<String>,
    /// The DOI printed on the PDF.
    pub detected_doi: Option<String>,
    /// Share of the report title's words found in the PDF title, from 0 to 1.
    pub title_overlap: Option<f64>,
    /// When the researcher chose to keep a PDF flagged as a mismatch.
    pub acknowledged_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct MissingFullTextDto {
    pub report_id: Uuid,
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    /// The report's DOI, used to look up an open-access copy.
    pub doi: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct FullTextQueueItemDto {
    pub report_id: Uuid,
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    pub doi: Option<String>,
    pub publication_year: Option<i32>,
    pub full_text_status: String,
    pub revision: i64,
    pub document: Option<DocumentDto>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct FullTextQueueDto {
    pub items: Vec<FullTextQueueItemDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct FullTextExclusionReasonDto {
    pub id: Uuid,
    pub code: String,
    pub label: String,
    pub stage: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct DocumentBlockDto {
    pub id: Uuid,
    pub document_id: Uuid,
    pub parser_version: String,
    pub page_number: i32,
    pub page_width: Option<f64>,
    pub page_height: Option<f64>,
    pub page_ocr_required: bool,
    pub kind: String,
    pub section_path: Vec<String>,
    pub ordinal: i32,
    pub text: String,
    #[schema(value_type = Option<Object>)]
    pub bbox: Option<serde_json::Value>,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct DocumentPageDto {
    pub document_id: Uuid,
    pub parser_version: String,
    pub page_number: i32,
    pub width: f64,
    pub height: f64,
    pub ocr_required: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct DocumentReferenceDto {
    pub id: Uuid,
    pub ordinal: i32,
    pub raw: String,
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub year: Option<i32>,
    pub venue: Option<String>,
    pub doi: Option<String>,
    /// `native` or `grobid`.
    pub source: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct DocumentSectionDto {
    pub id: Uuid,
    pub ordinal: i32,
    pub number: Option<String>,
    pub title: String,
    pub depth: i32,
    pub path: Vec<String>,
    /// `native` or `grobid`.
    pub source: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ExternalDocumentRequest {
    pub url: String,
    pub original_filename: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub(crate) struct DocumentLimit {
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub(crate) struct BlockQuery {
    pub q: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub(crate) struct FullTextQueueQuery {
    pub status: Option<String>,
    pub search: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

#[derive(ToSchema)]
#[allow(
    dead_code,
    reason = "schema-only multipart form used by OpenAPI generation"
)]
pub(crate) struct UploadDocumentForm {
    #[schema(format = Binary)]
    pub file: String,
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/reports/{report_id}/documents",
    operation_id = "listReportDocuments",
    tag = "documents",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path), DocumentLimit),
    responses((status = 200, body = Vec<DocumentDto>), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn list_documents(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<DocumentLimit>,
) -> Result<Json<Vec<DocumentDto>>, ApiError> {
    ensure_report_scope(&state.pool, project_id, report_id).await?;
    let documents = deepref_postgres::list_documents(
        &state.pool,
        project_id,
        Some(report_id),
        bounded_limit(query.limit)?,
    )
    .await?;
    Ok(Json(documents.into_iter().map(document_dto).collect()))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/screening/full-text",
    operation_id = "listFullTextScreeningQueue",
    tag = "documents",
    params(("project_id" = Uuid, Path), FullTextQueueQuery),
    responses((status = 200, body = FullTextQueueDto), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn list_full_text_queue(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<FullTextQueueQuery>,
) -> Result<Json<FullTextQueueDto>, ApiError> {
    ensure_project_scope(&state.pool, project_id).await?;
    if let Some(status) = query.status.as_deref()
        && !matches!(
            status,
            "not_required" | "unscreened" | "include" | "exclude" | "maybe"
        )
    {
        return Err(ApiError::BadRequest(
            "invalid full-text status filter".to_owned(),
        ));
    }
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if search.is_some_and(|value| value.len() > 200) {
        return Err(ApiError::BadRequest(
            "search must be at most 200 bytes".to_owned(),
        ));
    }
    let cursor = query
        .cursor
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| ApiError::BadRequest("invalid full-text queue cursor".to_owned()))?;
    let limit = bounded_limit(query.limit)?;
    let mut items = deepref_postgres::list_full_text_queue(
        &state.pool,
        project_id,
        query.status.as_deref(),
        search,
        cursor,
        limit + 1,
    )
    .await?;
    let has_more = items.len() as i64 > limit;
    items.truncate(limit as usize);
    let next_cursor = if has_more {
        items.last().map(|item| item.report_id.to_string())
    } else {
        None
    };
    Ok(Json(FullTextQueueDto {
        items: items
            .into_iter()
            .map(|item| FullTextQueueItemDto {
                report_id: item.report_id,
                title: item.title,
                abstract_text: item.abstract_text,
                doi: item.doi,
                publication_year: item.publication_year,
                full_text_status: item.full_text_status,
                revision: item.revision,
                document: item.document.map(document_dto),
            })
            .collect(),
        next_cursor,
    }))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/screening/full-text/missing",
    operation_id = "listMissingFullText",
    tag = "documents",
    params(("project_id" = Uuid, Path), DocumentLimit),
    responses((status = 200, body = Vec<MissingFullTextDto>), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn list_missing_full_text(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<DocumentLimit>,
) -> Result<Json<Vec<MissingFullTextDto>>, ApiError> {
    ensure_project_scope(&state.pool, project_id).await?;
    let items = deepref_postgres::list_missing_full_text(
        &state.pool,
        project_id,
        bounded_limit(query.limit)?,
    )
    .await?;
    Ok(Json(items.into_iter().map(missing_full_text_dto).collect()))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/screening/full-text/reasons",
    operation_id = "listFullTextExclusionReasons",
    tag = "documents",
    params(("project_id" = Uuid, Path)),
    responses((status = 200, body = Vec<FullTextExclusionReasonDto>), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn list_full_text_reasons(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<Vec<FullTextExclusionReasonDto>>, ApiError> {
    ensure_project_scope(&state.pool, project_id).await?;
    let reasons = deepref_postgres::list_full_text_reasons(&state.pool, project_id).await?;
    Ok(Json(
        reasons
            .into_iter()
            .map(|reason| FullTextExclusionReasonDto {
                id: reason.id,
                code: reason.code,
                label: reason.label,
                stage: reason.stage,
            })
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/reports/{report_id}/documents/{document_id}",
    operation_id = "getReportDocument",
    tag = "documents",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path), ("document_id" = Uuid, Path)),
    responses((status = 200, body = DocumentDto), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn get_document(
    State(state): State<AppState>,
    Path((project_id, report_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<DocumentDto>, ApiError> {
    Ok(Json(document_dto(
        deepref_postgres::get_document(&state.pool, project_id, report_id, document_id)
            .await
            .map_err(map_database_document_error)?,
    )))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/reports/{report_id}/documents/{document_id}/blocks",
    operation_id = "listDocumentBlocks",
    tag = "documents",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path), ("document_id" = Uuid, Path), BlockQuery),
    responses((status = 200, body = Vec<DocumentBlockDto>), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn list_document_blocks(
    State(state): State<AppState>,
    Path((project_id, report_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
    Query(query): Query<BlockQuery>,
) -> Result<Json<Vec<DocumentBlockDto>>, ApiError> {
    deepref_postgres::get_document(&state.pool, project_id, report_id, document_id)
        .await
        .map_err(map_database_document_error)?;
    let blocks = match query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
    {
        Some(search) => {
            if search.len() > 200 {
                return Err(ApiError::BadRequest(
                    "block search must be at most 200 bytes".to_owned(),
                ));
            }
            deepref_postgres::search_document_blocks(
                &state.pool,
                project_id,
                report_id,
                document_id,
                search,
                bounded_limit(query.limit)?,
            )
            .await?
        }
        None => {
            deepref_postgres::get_document_blocks(&state.pool, project_id, report_id, document_id)
                .await?
        }
    };
    Ok(Json(
        blocks
            .into_iter()
            .map(|block| DocumentBlockDto {
                id: block.id,
                document_id: block.document_id,
                parser_version: block.parser_version,
                page_number: block.page_number,
                page_width: block.page_width,
                page_height: block.page_height,
                page_ocr_required: block.page_ocr_required,
                kind: block.kind,
                section_path: block.section_path,
                ordinal: block.ordinal,
                text: block.text,
                bbox: block.bbox,
                content_hash: block.content_hash,
            })
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/reports/{report_id}/documents/{document_id}/pages",
    operation_id = "listDocumentPages",
    tag = "documents",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path), ("document_id" = Uuid, Path)),
    responses((status = 200, body = Vec<DocumentPageDto>), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn list_document_pages(
    State(state): State<AppState>,
    Path((project_id, report_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<Vec<DocumentPageDto>>, ApiError> {
    deepref_postgres::get_document(&state.pool, project_id, report_id, document_id)
        .await
        .map_err(map_database_document_error)?;
    let pages =
        deepref_postgres::get_document_pages(&state.pool, project_id, report_id, document_id)
            .await?;
    Ok(Json(
        pages
            .into_iter()
            .map(|page| DocumentPageDto {
                document_id: page.document_id,
                parser_version: page.parser_version,
                page_number: page.page_number,
                width: page.width,
                height: page.height,
                ocr_required: page.ocr_required,
            })
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/reports/{report_id}/documents/{document_id}/references",
    operation_id = "listDocumentReferences",
    tag = "documents",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path), ("document_id" = Uuid, Path)),
    responses((status = 200, body = Vec<DocumentReferenceDto>), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn list_document_references(
    State(state): State<AppState>,
    Path((project_id, report_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<Vec<DocumentReferenceDto>>, ApiError> {
    deepref_postgres::get_document(&state.pool, project_id, report_id, document_id)
        .await
        .map_err(map_database_document_error)?;
    let references =
        deepref_postgres::list_document_references(&state.pool, project_id, report_id, document_id)
            .await?;
    Ok(Json(
        references
            .into_iter()
            .map(|reference| DocumentReferenceDto {
                id: reference.id,
                ordinal: reference.ordinal,
                raw: reference.raw,
                title: reference.title,
                authors: reference.authors,
                year: reference.year,
                venue: reference.venue,
                doi: reference.doi,
                source: reference.source,
            })
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/reports/{report_id}/documents/{document_id}/sections",
    operation_id = "listDocumentSections",
    tag = "documents",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path), ("document_id" = Uuid, Path)),
    responses((status = 200, body = Vec<DocumentSectionDto>), (status = 404, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn list_document_sections(
    State(state): State<AppState>,
    Path((project_id, report_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<Vec<DocumentSectionDto>>, ApiError> {
    deepref_postgres::get_document(&state.pool, project_id, report_id, document_id)
        .await
        .map_err(map_database_document_error)?;
    let sections =
        deepref_postgres::list_document_sections(&state.pool, project_id, report_id, document_id)
            .await?;
    Ok(Json(
        sections
            .into_iter()
            .map(|section| DocumentSectionDto {
                id: section.id,
                ordinal: section.ordinal,
                number: section.number,
                title: section.title,
                depth: section.depth,
                path: section.path,
                source: section.source,
            })
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/reports/{report_id}/documents/{document_id}/content",
    operation_id = "streamReportDocumentContent",
    tag = "documents",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path), ("document_id" = Uuid, Path)),
    responses((status = 200, description = "Project-scoped streamed PDF content"), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse))
)]
pub(crate) async fn get_document_content(
    State(state): State<AppState>,
    Path((project_id, report_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Response, ApiError> {
    let document = deepref_postgres::get_document(&state.pool, project_id, report_id, document_id)
        .await
        .map_err(map_database_document_error)?;
    let object_key = document.object_key.ok_or_else(|| ApiError::Conflict {
        code: "document_content_unavailable".to_owned(),
        message: "document content is not available".to_owned(),
        details: serde_json::json!({ "status": document.status }),
    })?;
    let store = state
        .document_store
        .ok_or_else(|| ApiError::Configuration("document storage is not configured".to_owned()))?;
    let object = store.get(&object_key).await.map_err(|error| {
        if error.is_not_found() {
            ApiError::Conflict {
                code: deepref_documents::DOCUMENT_BLOB_MISSING_CODE.to_owned(),
                message: "The stored PDF file is no longer available. Upload the PDF again."
                    .to_owned(),
                details: serde_json::json!({}),
            }
        } else {
            ApiError::Internal(error.into())
        }
    })?;
    let mut response = Body::from_stream(object.into_stream()).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&document.mime_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/pdf")),
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&document.byte_size.to_string())
            .map_err(|error| ApiError::Internal(error.into()))?,
    );
    response
        .headers_mut()
        .insert(header::ACCEPT_RANGES, HeaderValue::from_static("none"));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    let disposition = document
        .original_filename
        .as_deref()
        .map(|name| format!("inline; filename=\"{}\"", name.replace(['\"', '\\'], "_")))
        .unwrap_or_else(|| "inline".to_owned());
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition).map_err(|error| ApiError::Internal(error.into()))?,
    );
    Ok(response)
}

async fn stream_pdf_to_store(
    mut field: axum::extract::multipart::Field<'_>,
    store: &deepref_documents::DocumentStore,
) -> Result<deepref_documents::StoredObject, ApiError> {
    let mut prefix = Vec::with_capacity(5);
    let mut buffered = Vec::new();
    while prefix.len() < 5 {
        let chunk = field
            .next()
            .await
            .ok_or_else(|| ApiError::BadRequest("PDF body is empty".to_owned()))?
            .map_err(|error| ApiError::BadRequest(error.to_string()))?;
        let needed = 5 - prefix.len();
        prefix.extend_from_slice(&chunk[..chunk.len().min(needed)]);
        buffered.push(chunk);
    }
    if prefix.as_slice() != b"%PDF-" {
        return Err(ApiError::BadRequest(
            "document does not have a PDF signature".to_owned(),
        ));
    }
    let upload_stream = stream::iter(buffered.into_iter().map(Ok::<Bytes, String>))
        .chain(field.map_err(|error| error.to_string()));
    store
        .put_stream(upload_stream)
        .await
        .map_err(map_store_error)
}

async fn extract_multipart_pdf(
    mut multipart: Multipart,
    store: &deepref_documents::DocumentStore,
) -> Result<(deepref_documents::StoredObject, Option<String>), ApiError> {
    let mut stored: Option<deepref_documents::StoredObject> = None;
    let mut original_filename = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => {
                if let Some(existing) = stored.take() {
                    let _ = store.delete(&existing.opaque_id).await;
                }
                return Err(ApiError::BadRequest(error.to_string()));
            }
        };
        if field.name() == Some("file") {
            if stored.is_some() {
                if let Some(existing) = stored.take() {
                    let _ = store.delete(&existing.opaque_id).await;
                }
                return Err(ApiError::BadRequest("only one file is accepted".to_owned()));
            }
            if field.content_type() != Some("application/pdf") {
                return Err(ApiError::BadRequest(
                    "document MIME type must be application/pdf".to_owned(),
                ));
            }
            original_filename = sanitize_filename(field.file_name());
            match stream_pdf_to_store(field, store).await {
                Ok(obj) => stored = Some(obj),
                Err(err) => {
                    if let Some(existing) = stored.take() {
                        let _ = store.delete(&existing.opaque_id).await;
                    }
                    return Err(err);
                }
            }
        }
    }
    let stored = stored
        .ok_or_else(|| ApiError::BadRequest("multipart field file is required".to_owned()))?;
    Ok((stored, original_filename))
}

async fn persist_uploaded_document(
    pool: &sqlx::PgPool,
    store: &deepref_documents::DocumentStore,
    project_id: Uuid,
    report_id: Uuid,
    stored: &deepref_documents::StoredObject,
    original_filename: Option<&str>,
    actor: &super::actor::Actor,
) -> Result<DocumentDto, ApiError> {
    let byte_size = i64::try_from(stored.byte_size)
        .map_err(|_| ApiError::BadRequest("document size exceeds supported limit".to_owned()))?;
    let mut tx = match pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            let _ = store.delete(&stored.opaque_id).await;
            return Err(ApiError::Database(error));
        }
    };
    let result = deepref_postgres::create_document(
        &mut tx,
        deepref_postgres::NewDocument {
            project_id,
            report_id,
            id: Uuid::new_v4(),
            source: "upload",
            status: "uploaded",
            original_filename,
            external_url: None,
            mime_type: "application/pdf",
            byte_size,
            content_hash: Some(&stored.sha256),
            object_key: Some(&stored.opaque_id),
            actor_kind: actor.kind().as_str(),
            actor_id: actor.id(),
        },
    )
    .await;
    let document = match result {
        Ok(document) => {
            match deepref_postgres::enqueue_parse(
                &mut tx,
                ProjectId::new(project_id),
                document.id,
                &stored.sha256,
            )
            .await
            {
                Ok(_) => match tx.commit().await {
                    Ok(()) => document,
                    Err(error) => {
                        let _ = store.delete(&stored.opaque_id).await;
                        return Err(ApiError::Database(error));
                    }
                },
                Err(error) => {
                    let _ = tx.rollback().await;
                    let _ = store.delete(&stored.opaque_id).await;
                    return Err(map_database_document_error(error));
                }
            }
        }
        Err(error) => {
            let _ = store.delete(&stored.opaque_id).await;
            let _ = tx.rollback().await;
            return Err(map_database_document_error(error));
        }
    };
    Ok(document_dto(document))
}
#[utoipa::path(
    post,
    path = "/projects/{project_id}/reports/{report_id}/documents",
    operation_id = "uploadReportDocument",
    tag = "documents",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path)),
    request_body(content = UploadDocumentForm, content_type = "multipart/form-data", description = "Multipart form with a binary PDF field named file"),
    responses((status = 201, body = DocumentDto), (status = 400, body = ErrorResponse), (status = 413, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn upload_document(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Result<(StatusCode, Json<DocumentDto>), ApiError> {
    ensure_report_scope(&state.pool, project_id, report_id).await?;
    let actor = super::actor::extract_actor(&headers)?;
    let store = state
        .document_store
        .as_ref()
        .ok_or_else(|| ApiError::Configuration("document storage is not configured".to_owned()))?;
    let (stored, original_filename) = extract_multipart_pdf(multipart, store).await?;
    // Re-uploading identical content for a document whose processing failed
    // retries that document instead of colliding with the unique content hash.
    if let Some(existing) = deepref_postgres::find_failed_document_by_hash(
        &state.pool,
        project_id,
        report_id,
        &stored.sha256,
    )
    .await
    .map_err(map_database_document_error)?
    {
        let _ = store.delete(&stored.opaque_id).await;
        let document =
            requeue_document(&state.pool, project_id, report_id, existing, &actor).await?;
        return Ok((StatusCode::CREATED, Json(document)));
    }
    let document = persist_uploaded_document(
        &state.pool,
        store,
        project_id,
        report_id,
        &stored,
        original_filename.as_deref(),
        &actor,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(document)))
}

async fn requeue_document(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    report_id: Uuid,
    document_id: Uuid,
    actor: &super::actor::Actor,
) -> Result<DocumentDto, ApiError> {
    let mut tx = pool.begin().await?;
    // Record who asked for the reprocessing without touching the document's
    // creator (actor_kind / actor_id); rolled back with the transaction when
    // the document turns out not to be retryable.
    sqlx::query(
        "UPDATE documents SET reparsed_by_kind=$4, reparsed_by_id=$5, reparsed_at=now()
         WHERE id=$1 AND project_id=$2 AND report_id=$3",
    )
    .bind(document_id)
    .bind(project_id)
    .bind(report_id)
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .execute(&mut *tx)
    .await?;
    let document = deepref_postgres::requeue_document_processing(
        &mut tx,
        ProjectId::new(project_id),
        report_id,
        document_id,
    )
    .await
    .map_err(map_database_document_error)?
    .ok_or_else(|| ApiError::Conflict {
        code: "document_not_retryable".to_owned(),
        message: "this document cannot be reprocessed in its current state".to_owned(),
        details: serde_json::json!({}),
    })?;
    tx.commit().await?;
    Ok(document_dto(document))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/reports/{report_id}/documents/{document_id}/reparse",
    operation_id = "reparseReportDocument",
    tag = "documents",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path), ("document_id" = Uuid, Path)),
    responses((status = 202, body = DocumentDto), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn reparse_document(
    State(state): State<AppState>,
    Path((project_id, report_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<DocumentDto>), ApiError> {
    let actor = super::actor::extract_actor(&headers)?;
    ensure_report_scope(&state.pool, project_id, report_id).await?;
    let stored = deepref_postgres::get_document(&state.pool, project_id, report_id, document_id)
        .await
        .map_err(map_database_document_error)?;
    // Refuse before queueing: a parse job cannot succeed without the PDF bytes,
    // and the caller needs a clear "upload it again" answer, not a failed job.
    if let Some(object_key) = stored.object_key.as_deref() {
        let store = state.document_store.as_ref().ok_or_else(|| {
            ApiError::Configuration("document storage is not configured".to_owned())
        })?;
        let present = store
            .exists(object_key)
            .await
            .map_err(|error| ApiError::Internal(error.into()))?;
        if !present {
            return Err(ApiError::Conflict {
                code: deepref_documents::DOCUMENT_BLOB_MISSING_CODE.to_owned(),
                message: "The stored PDF file is no longer available. Upload the PDF again to re-parse it.".to_owned(),
                details: serde_json::json!({}),
            });
        }
    }
    let document =
        requeue_document(&state.pool, project_id, report_id, document_id, &actor).await?;
    Ok((StatusCode::ACCEPTED, Json(document)))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/reports/{report_id}/documents/external",
    operation_id = "attachExternalReportDocument",
    tag = "documents",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path)),
    request_body = ExternalDocumentRequest,
    responses((status = 201, body = DocumentDto), (status = 400, body = ErrorResponse), (status = 413, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn attach_external_document(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(input): Json<ExternalDocumentRequest>,
) -> Result<(StatusCode, Json<DocumentDto>), ApiError> {
    ensure_report_scope(&state.pool, project_id, report_id).await?;
    let actor = super::actor::extract_actor(&headers)?;
    let url =
        deepref_documents::validate_external_url(&input.url).map_err(map_remote_fetch_error)?;
    let original_filename = sanitize_filename(input.original_filename.as_deref());
    let document = attach_external_url(
        &state.pool,
        project_id,
        report_id,
        url.as_str(),
        original_filename.as_deref(),
        &actor,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(document_dto(document))))
}

/// Records an external PDF address on the report and queues its retrieval.
async fn attach_external_url(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    report_id: Uuid,
    url: &str,
    original_filename: Option<&str>,
    actor: &super::actor::Actor,
) -> Result<deepref_postgres::DocumentRecord, ApiError> {
    let mut tx = pool.begin().await?;
    let document = deepref_postgres::create_document(
        &mut tx,
        deepref_postgres::NewDocument {
            project_id,
            report_id,
            id: Uuid::new_v4(),
            source: "external_url",
            status: "external",
            original_filename,
            external_url: Some(url),
            mime_type: "application/pdf",
            byte_size: 0,
            content_hash: None,
            object_key: None,
            actor_kind: actor.kind().as_str(),
            actor_id: actor.id(),
        },
    )
    .await;
    match document {
        Ok(document) => match deepref_postgres::enqueue_retrieve(
            &mut tx,
            ProjectId::new(project_id),
            document.id,
        )
        .await
        {
            Ok(_) => {
                tx.commit().await?;
                Ok(document)
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(map_database_document_error(error))
            }
        },
        Err(error) => {
            tx.rollback().await?;
            Err(map_database_document_error(error))
        }
    }
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/reports/{report_id}/documents/open-access",
    operation_id = "attachOpenAccessReportDocument",
    tag = "documents",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path)),
    responses((status = 201, body = DocumentDto), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse), (status = 502, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn attach_open_access_document(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<DocumentDto>), ApiError> {
    ensure_report_scope(&state.pool, project_id, report_id).await?;
    let actor = super::actor::extract_actor(&headers)?;
    let (_, doi) = deepref_postgres::report_identity(&state.pool, report_id).await?;
    let doi = doi.ok_or_else(|| ApiError::Conflict {
        code: "report_doi_missing".to_owned(),
        message: "This report has no DOI, so an open-access copy cannot be looked up. Upload the PDF or paste a link instead.".to_owned(),
        details: serde_json::json!({}),
    })?;
    // The workspace's contact address identifies us to Unpaywall; a personal address is never used.
    let contact: String = sqlx::query_scalar("SELECT crossref_mailto FROM settings WHERE id=1")
        .fetch_optional(&state.pool)
        .await?
        .unwrap_or_default();
    if contact.trim().is_empty() {
        return Err(missing_contact_email());
    }
    let lookup = deepref_crossref::UnpaywallClient::new(contact)
        .map_err(|error| ApiError::Internal(error.into()))?;
    let pdf_url = lookup
        .open_access_pdf_url(&doi)
        .await
        .map_err(map_unpaywall_error)?;
    let Some(pdf_url) = pdf_url else {
        return Err(ApiError::NotFound(
            "No open-access PDF is listed for this DOI. Upload the PDF or paste a link instead."
                .to_owned(),
        ));
    };
    let url = deepref_documents::validate_external_url(&pdf_url).map_err(map_remote_fetch_error)?;
    let document = attach_external_url(
        &state.pool,
        project_id,
        report_id,
        url.as_str(),
        None,
        &actor,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(document_dto(document))))
}

fn missing_contact_email() -> ApiError {
    ApiError::Conflict {
        code: "contact_email_missing".to_owned(),
        message: "Set a contact e-mail in Settings first. Unpaywall asks for one before it looks up open-access PDFs.".to_owned(),
        details: serde_json::json!({}),
    }
}

fn map_unpaywall_error(error: deepref_crossref::UnpaywallError) -> ApiError {
    use deepref_crossref::UnpaywallError;
    match error {
        UnpaywallError::MissingEmail => missing_contact_email(),
        UnpaywallError::Rejected(status) if status == StatusCode::UNPROCESSABLE_ENTITY => {
            ApiError::Conflict {
                code: "contact_email_rejected".to_owned(),
                message: "Unpaywall did not accept the contact e-mail in Settings. Check that it is a real address.".to_owned(),
                details: serde_json::json!({}),
            }
        }
        UnpaywallError::InvalidDoi(error) => ApiError::Doi(error),
        UnpaywallError::Rejected(_)
        | UnpaywallError::Unavailable(_)
        | UnpaywallError::Request(_)
        | UnpaywallError::Json(_) => ApiError::Upstream(
            "The open-access lookup service did not answer. Try again later.".to_owned(),
        ),
    }
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/reports/{report_id}/documents/{document_id}/identity/acknowledge",
    operation_id = "acknowledgeReportDocumentIdentity",
    tag = "documents",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path), ("document_id" = Uuid, Path)),
    responses((status = 200, body = DocumentDto), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn acknowledge_document_identity(
    State(state): State<AppState>,
    Path((project_id, report_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<DocumentDto>, ApiError> {
    ensure_report_scope(&state.pool, project_id, report_id).await?;
    let document = deepref_postgres::get_document(&state.pool, project_id, report_id, document_id)
        .await
        .map_err(map_database_document_error)?;
    let flagged = document
        .identity_check
        .as_ref()
        .is_some_and(|check| check.verdict == deepref_documents::IdentityVerdict::Mismatch);
    if !flagged {
        return Err(ApiError::Conflict {
            code: "document_not_flagged".to_owned(),
            message: "This PDF has no possible-mismatch flag to keep.".to_owned(),
            details: serde_json::json!({}),
        });
    }
    deepref_postgres::acknowledge_identity_mismatch(
        &state.pool,
        project_id,
        report_id,
        document_id,
    )
    .await?;
    let document = deepref_postgres::get_document(&state.pool, project_id, report_id, document_id)
        .await
        .map_err(map_database_document_error)?;
    Ok(Json(document_dto(document)))
}

#[utoipa::path(
    delete,
    path = "/projects/{project_id}/reports/{report_id}/documents/{document_id}",
    operation_id = "deleteReportDocument",
    tag = "documents",
    params(("project_id" = Uuid, Path), ("report_id" = Uuid, Path), ("document_id" = Uuid, Path)),
    responses((status = 204, description = "The document and its stored file were removed"), (status = 404, body = ErrorResponse), (status = 409, body = ErrorResponse), (status = 500, body = ErrorResponse))
)]
pub(crate) async fn delete_report_document(
    State(state): State<AppState>,
    Path((project_id, report_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    ensure_report_scope(&state.pool, project_id, report_id).await?;
    let removed =
        deepref_postgres::delete_document(&state.pool, project_id, report_id, document_id)
            .await
            .map_err(|error| {
                // AI evidence and extracted values refer to the PDF's blocks and are kept on purpose.
                if let Some(SqlxError::Database(database)) = error.downcast_ref::<SqlxError>()
                    && database.code().as_deref() == Some("23503")
                {
                    return ApiError::Conflict {
                        code: "document_in_use".to_owned(),
                        message: "This PDF supports AI suggestions or extracted values, so it cannot be removed. Remove those first.".to_owned(),
                        details: serde_json::json!({}),
                    };
                }
                map_database_document_error(error)
            })?
            .ok_or_else(|| ApiError::NotFound("document not found".to_owned()))?;
    if let (Some(object_key), Some(store)) = (removed, state.document_store.as_ref())
        && let Err(error) = store.delete(&object_key).await
    {
        tracing::warn!(%document_id, %error, "removed document row, but its stored file could not be deleted");
    }
    Ok(StatusCode::NO_CONTENT)
}

fn bounded_limit(limit: Option<i64>) -> Result<i64, ApiError> {
    let limit = limit.unwrap_or(50);
    if !(1..=MAX_DOCUMENT_LIST_LIMIT).contains(&limit) {
        return Err(ApiError::BadRequest(
            "limit must be between 1 and 100".to_owned(),
        ));
    }
    Ok(limit)
}

async fn ensure_project_scope(pool: &sqlx::PgPool, project_id: Uuid) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
        .bind(project_id)
        .fetch_one(pool)
        .await?;
    if !exists {
        return Err(ApiError::NotFound("project not found".to_owned()));
    }
    Ok(())
}

async fn ensure_report_scope(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    report_id: Uuid,
) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM project_reports WHERE project_id=$1 AND report_id=$2)",
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_one(pool)
    .await?;
    if !exists {
        return Err(ApiError::NotFound("report not found in project".to_owned()));
    }
    Ok(())
}

fn document_dto(document: deepref_postgres::DocumentRecord) -> DocumentDto {
    let identity = document.identity_check.map(|check| DocumentIdentityDto {
        verdict: match check.verdict {
            deepref_documents::IdentityVerdict::Match => "match",
            deepref_documents::IdentityVerdict::Mismatch => "mismatch",
            deepref_documents::IdentityVerdict::Unknown => "unknown",
        }
        .to_owned(),
        detected_title: check.detected_title,
        detected_doi: check.detected_doi,
        title_overlap: check.title_overlap,
        acknowledged_at: document.identity_acknowledged_at,
    });
    DocumentDto {
        id: document.id,
        report_id: document.report_id,
        original_filename: document.original_filename,
        source: document.source,
        status: document.status,
        external_url: document.external_url,
        mime_type: document.mime_type,
        byte_size: document.byte_size,
        content_hash: document.content_hash,
        parser_version: document.parser_version,
        parser_error: document.parser_error,
        ocr_required: document.ocr_required,
        identity,
        created_at: document.created_at,
        updated_at: document.updated_at,
    }
}

fn missing_full_text_dto(item: deepref_postgres::MissingFullTextRecord) -> MissingFullTextDto {
    MissingFullTextDto {
        report_id: item.report_id,
        title: item.title,
        abstract_text: item.abstract_text,
        doi: item.doi,
        status: item.status,
    }
}

fn sanitize_filename(filename: Option<&str>) -> Option<String> {
    let filename = filename?
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    let filename = filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim();
    (!filename.is_empty() && filename.len() <= MAX_FILENAME_BYTES).then(|| filename.to_owned())
}

fn map_store_error(error: deepref_documents::DocumentStoreError) -> ApiError {
    match error {
        deepref_documents::DocumentStoreError::TooLarge { maximum, .. } => {
            ApiError::PayloadTooLarge(format!("document exceeds {maximum} bytes"))
        }
        other => ApiError::Internal(other.into()),
    }
}

fn map_remote_fetch_error(error: deepref_documents::RemoteFetchError) -> ApiError {
    match error {
        deepref_documents::RemoteFetchError::TooLarge => {
            ApiError::PayloadTooLarge("external document is too large".to_owned())
        }
        deepref_documents::RemoteFetchError::Store(store_error) => map_store_error(store_error),
        deepref_documents::RemoteFetchError::Request(_) => {
            ApiError::BadRequest("external document request failed".to_owned())
        }
        other => ApiError::BadRequest(other.to_string()),
    }
}

fn map_database_document_error(error: anyhow::Error) -> ApiError {
    if error
        .downcast_ref::<SqlxError>()
        .is_some_and(|error| matches!(error, SqlxError::RowNotFound))
    {
        return ApiError::NotFound("document not found".to_owned());
    }
    if let Some(SqlxError::Database(database)) = error.downcast_ref::<SqlxError>()
        && database
            .constraint()
            .is_some_and(|constraint| constraint.contains("content_hash"))
    {
        return ApiError::Conflict {
            code: "document_content_duplicate".to_owned(),
            message: "the same document content is already attached to this report".to_owned(),
            details: serde_json::json!({}),
        };
    }
    if let Some(SqlxError::Database(database)) = error.downcast_ref::<SqlxError>()
        && database.constraint() == Some("documents_report_external_url_uq")
    {
        return ApiError::Conflict {
            code: "document_url_duplicate".to_owned(),
            message: "this external document URL is already attached to the report".to_owned(),
            details: serde_json::json!({}),
        };
    }
    if let Some(SqlxError::Database(database)) = error.downcast_ref::<SqlxError>()
        && database.constraint() == Some("documents_project_report_fk")
    {
        return ApiError::NotFound("report not found in project".to_owned());
    }
    ApiError::Internal(error)
}
