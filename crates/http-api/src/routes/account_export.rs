//! HTTP routes for self-service account data export ("Download my data").

use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use deepref_app::account_export::{
    AccountExportRecord, ExportError, ExportStatus, authorize_download, cancel_account_export,
    get_latest_account_export, lock_download_read, record_export_download_audit,
    request_account_export,
};
use deepref_app::identity::AuditRequest;
use deepref_domain::UserId;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    auth::CurrentSession,
    error::{ApiError, ApiErrorBody, AuthFailure},
    state::AppState,
};

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AccountExportResponse {
    pub id: Uuid,
    pub status: String,
    pub requested_at: String,
    pub completed_at: Option<String>,
    pub cancelled_at: Option<String>,
    pub expires_at: Option<String>,
    pub retained_until: Option<String>,
    pub size_bytes: Option<i64>,
    pub sha256: Option<String>,
    pub failure_reason: Option<String>,
    pub scope_manifest: Option<serde_json::Value>,
}

impl From<AccountExportRecord> for AccountExportResponse {
    fn from(e: AccountExportRecord) -> Self {
        Self {
            id: e.id,
            status: e.status.as_str().to_owned(),
            requested_at: e.requested_at.to_rfc3339(),
            completed_at: e.completed_at.map(|t| t.to_rfc3339()),
            cancelled_at: e.cancelled_at.map(|t| t.to_rfc3339()),
            expires_at: e.expires_at.map(|t| t.to_rfc3339()),
            retained_until: e.retained_until.map(|t| t.to_rfc3339()),
            size_bytes: e.size_bytes,
            sha256: e.sha256,
            failure_reason: e.failure_reason,
            scope_manifest: e.scope_manifest,
        }
    }
}

fn account_owner(state: &AppState, session: Option<&CurrentSession>) -> Result<UserId, ApiError> {
    if state.auth.is_none() {
        return Err(ApiError::NotFound(
            "authentication is not enabled".to_owned(),
        ));
    }
    let CurrentSession(context) = session.ok_or(ApiError::Auth(AuthFailure::Unauthenticated))?;
    context
        .principal
        .user_id()
        .ok_or_else(|| ApiError::NotFound("resource not found".to_owned()))
}

fn map_export_error(error: ExportError) -> ApiError {
    match error {
        ExportError::RetentionNotActivated => ApiError::Conflict {
            code: "export_retention_not_activated".to_owned(),
            message: "Account export retention is not activated by operator".to_owned(),
            details: serde_json::json!({}),
        },
        ExportError::AccountNotActive => ApiError::Forbidden("account is not active".to_owned()),
        ExportError::ActiveExportExists => ApiError::Conflict {
            code: "active_export_exists".to_owned(),
            message: "an export request is already active or in progress".to_owned(),
            details: serde_json::json!({}),
        },
        ExportError::NotFound => ApiError::NotFound("export not found".to_owned()),
        ExportError::NotCompleted => ApiError::Conflict {
            code: "export_not_completed".to_owned(),
            message: "export is not in completed state".to_owned(),
            details: serde_json::json!({}),
        },
        ExportError::Expired => ApiError::Conflict {
            code: "export_expired".to_owned(),
            message: "export has expired".to_owned(),
            details: serde_json::json!({}),
        },
        ExportError::Cancelled => ApiError::Conflict {
            code: "export_cancelled".to_owned(),
            message: "export was cancelled".to_owned(),
            details: serde_json::json!({}),
        },
        ExportError::Database(err) => ApiError::Database(err),
        ExportError::Other(err) => ApiError::Internal(err),
    }
}

#[utoipa::path(
    post,
    path = "/me/export",
    operation_id = "requestAccountExport",
    tag = "auth",
    responses(
        (status = 202, body = AccountExportResponse, description = "Export requested"),
        (status = 401, body = ApiErrorBody, description = "No valid session"),
        (status = 404, body = ApiErrorBody, description = "Authentication is not enabled"),
        (status = 409, body = ApiErrorBody, description = "Export retention is not activated"),
    )
)]
pub async fn request_export(
    State(state): State<AppState>,
    session: Option<CurrentSession>,
) -> Result<(StatusCode, Json<AccountExportResponse>), ApiError> {
    let user = account_owner(&state, session.as_ref())?;
    let request_id = crate::error::correlation_id();
    let export = request_account_export(
        &state.pool,
        user,
        AuditRequest {
            request_id: request_id.as_deref(),
            session_ref: None,
        },
    )
    .await
    .map_err(map_export_error)?;

    Ok((StatusCode::ACCEPTED, Json(export.into())))
}

#[utoipa::path(
    get,
    path = "/me/export",
    operation_id = "getAccountExportStatus",
    tag = "auth",
    responses(
        (status = 200, body = Option<AccountExportResponse>, description = "Latest export status"),
        (status = 401, body = ApiErrorBody, description = "No valid session"),
        (status = 404, body = ApiErrorBody, description = "Authentication is not enabled"),
    )
)]
pub async fn get_export_status(
    State(state): State<AppState>,
    session: Option<CurrentSession>,
) -> Result<Json<Option<AccountExportResponse>>, ApiError> {
    let user = account_owner(&state, session.as_ref())?;
    let export = get_latest_account_export(&state.pool, user)
        .await
        .map_err(map_export_error)?;

    Ok(Json(export.map(Into::into)))
}

#[utoipa::path(
    get,
    path = "/me/export/{id}/download",
    operation_id = "downloadAccountExport",
    tag = "auth",
    params(
        ("id" = Uuid, Path, description = "The export UUID")
    ),
    responses(
        (status = 200, description = "Streamed ZIP archive of account data"),
        (status = 401, body = ApiErrorBody, description = "No valid session"),
        (status = 404, body = ApiErrorBody, description = "Export not found"),
        (status = 409, body = ApiErrorBody, description = "Export is still processing"),
    )
)]
pub async fn download_export(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    session: Option<CurrentSession>,
) -> Result<Response, ApiError> {
    let started = tokio::time::Instant::now();
    let user = account_owner(&state, session.as_ref())?;
    let preparation_deadline = started + std::time::Duration::from_secs(300);
    let export = tokio::time::timeout_at(
        preparation_deadline,
        authorize_download(&state.pool, user, id),
    )
    .await
    .map_err(|_| download_timeout())?
    .map_err(map_export_error)?;
    let retention_remaining = export
        .retained_until
        .and_then(|until| (until - chrono::Utc::now()).to_std().ok())
        .unwrap_or_default();
    let deadline = download_deadline(started, retention_remaining);
    if tokio::time::Instant::now() >= deadline {
        return Err(download_timeout());
    }

    match export.status {
        ExportStatus::Completed => {}
        ExportStatus::Requested | ExportStatus::Processing => {
            return Err(ApiError::Conflict {
                code: "export_in_progress".to_owned(),
                message: "Export packaging is still in progress".to_owned(),
                details: serde_json::json!({ "status": export.status.as_str() }),
            });
        }
        ExportStatus::Cancelled | ExportStatus::Expired => {
            return Err(ApiError::Conflict {
                code: "export_cancelled".to_owned(),
                message: "export was cancelled".to_owned(),
                details: serde_json::json!({}),
            });
        }
        ExportStatus::Failed => {
            return Err(ApiError::Internal(anyhow::anyhow!(
                "export failed: {}",
                export.failure_reason.unwrap_or_default()
            )));
        }
    }

    let storage_key = export.storage_key.ok_or_else(|| {
        ApiError::Internal(anyhow::anyhow!("completed export missing storage key"))
    })?;

    let store = state
        .document_store
        .ok_or_else(|| ApiError::Configuration("document storage is not configured".to_owned()))?;

    let read = tokio::time::timeout_at(
        deadline,
        lock_download_read(&state.pool, user, id, &storage_key),
    )
    .await
    .map_err(|_| download_timeout())?
    .map_err(map_export_error)?;
    let object = tokio::time::timeout_at(deadline, store.get(&storage_key))
        .await
        .map_err(|_| download_timeout())?
        .map_err(|error| {
            if error.is_not_found() {
                ApiError::NotFound("export artifact is no longer available".to_owned())
            } else {
                ApiError::Internal(error.into())
            }
        })?;
    // Object acquisition is serialized with erasure. Release this connection
    // before the audit call, including when the business pool has size one.
    tokio::time::timeout_at(deadline, read.commit())
        .await
        .map_err(|_| download_timeout())??;

    // Record audit event for download
    let request_id = crate::error::correlation_id();
    tokio::time::timeout_at(
        deadline,
        record_export_download_audit(
            &state.pool,
            user,
            id,
            AuditRequest {
                request_id: request_id.as_deref(),
                session_ref: None,
            },
        ),
    )
    .await
    .map_err(|_| download_timeout())?
    .map_err(map_export_error)?;
    // timeout_at can accept a ready future at the boundary: refuse a body
    // after expired preparation even when no operation needed another poll.
    if tokio::time::Instant::now() >= deadline {
        return Err(download_timeout());
    }
    let stream = bounded_download_stream(
        revocable_download_stream(
            state.pool,
            user,
            id,
            storage_key,
            object
                .into_stream()
                .map(|value| value.map_err(std::io::Error::other)),
        ),
        deadline,
    );
    let mut response = Body::from_stream(stream).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, private"),
    );
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    if let Some(size) = export.size_bytes
        && let Ok(header_val) = HeaderValue::from_str(&size.to_string())
    {
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, header_val);
    }
    let filename = format!("attachment; filename=\"account-export-{id}.zip\"");
    if let Ok(header_val) = HeaderValue::from_str(&filename) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, header_val);
    }

    Ok(response)
}

#[utoipa::path(
    delete,
    path = "/me/export/{id}",
    operation_id = "cancelAccountExport",
    tag = "auth",
    params(
        ("id" = Uuid, Path, description = "The export UUID")
    ),
    responses(
        (status = 204, description = "Export cancelled"),
        (status = 401, body = ApiErrorBody, description = "No valid session"),
        (status = 404, body = ApiErrorBody, description = "Export not found"),
    )
)]
pub async fn cancel_export(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    session: Option<CurrentSession>,
) -> Result<StatusCode, ApiError> {
    let user = account_owner(&state, session.as_ref())?;
    let request_id = crate::error::correlation_id();
    cancel_account_export(
        &state.pool,
        user,
        id,
        AuditRequest {
            request_id: request_id.as_deref(),
            session_ref: None,
        },
    )
    .await
    .map_err(map_export_error)?;

    Ok(StatusCode::NO_CONTENT)
}

fn download_deadline(
    started: tokio::time::Instant,
    retention_remaining: std::time::Duration,
) -> tokio::time::Instant {
    (started + std::time::Duration::from_secs(300))
        .min(tokio::time::Instant::now() + retention_remaining)
}

fn download_timeout() -> ApiError {
    ApiError::Internal(anyhow::anyhow!("export download deadline exceeded"))
}

fn revocable_download_stream<S>(
    pool: sqlx::PgPool,
    user: UserId,
    id: Uuid,
    storage_key: String,
    source: S,
) -> impl Stream<Item = std::io::Result<Bytes>> + Send
where
    S: Stream<Item = std::io::Result<Bytes>> + Send + 'static,
{
    futures::stream::unfold(
        (Box::pin(source), pool, storage_key, false),
        move |(mut source, pool, key, done)| async move {
            if done {
                return None;
            }
            let read = lock_download_read(&pool, user, id, &key).await;
            let Ok(read) = read else {
                return Some((
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "export is no longer available",
                    )),
                    (source, pool, key, true),
                ));
            };
            // Keep the fence until the adapter has finished this read. The
            // outer absolute response deadline bounds this wait and DB locks.
            let value = source.next().await;
            if let Err(error) = read.commit().await {
                return Some((Err(std::io::Error::other(error)), (source, pool, key, true)));
            }
            value.map(|value| (value, (source, pool, key, false)))
        },
    )
}

fn bounded_download_stream<S>(
    source: S,
    deadline: tokio::time::Instant,
) -> impl Stream<Item = std::io::Result<Bytes>> + Send
where
    S: Stream<Item = std::io::Result<Bytes>> + Send + 'static,
{
    futures::stream::unfold(
        (Box::pin(source), false),
        move |(mut source, done)| async move {
            if done {
                return None;
            }
            if tokio::time::Instant::now() >= deadline {
                return Some((
                    Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "export download deadline exceeded",
                    )),
                    (source, true),
                ));
            }
            match tokio::time::timeout_at(deadline, source.next()).await {
                Ok(Some(value)) if tokio::time::Instant::now() < deadline => {
                    Some((value, (source, false)))
                }
                Ok(None) => None,
                Ok(Some(_)) | Err(_) => Some((
                    Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "export download deadline exceeded",
                    )),
                    (source, true),
                )),
            }
        },
    )
}
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod deadline_tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn chunk_ready_after_deadline_is_refused() {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        let source = futures::stream::once(async {
            tokio::time::advance(std::time::Duration::from_secs(2)).await;
            Ok(Bytes::from_static(b"late ready chunk"))
        });
        let body = bounded_download_stream(source, deadline);
        futures::pin_mut!(body);
        assert_eq!(
            body.next().await.unwrap().unwrap_err().kind(),
            std::io::ErrorKind::TimedOut
        );
    }
    #[tokio::test(start_paused = true)]
    async fn delayed_download_preparation_cannot_extend_retained_access() {
        let started = tokio::time::Instant::now();
        // Object acquisition plus audit use 299s of a 301s retained window.
        tokio::time::sleep(std::time::Duration::from_secs(200)).await;
        tokio::time::sleep(std::time::Duration::from_secs(99)).await;
        let deadline = download_deadline(started, std::time::Duration::from_secs(2));
        let source = futures::stream::once(async {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            Ok(Bytes::from_static(b"must-not-outlive-retention"))
        });
        let body = bounded_download_stream(source, deadline);
        futures::pin_mut!(body);
        let result = body.next().await.unwrap();
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
        assert!(tokio::time::Instant::now() <= started + std::time::Duration::from_secs(301));
    }
}
