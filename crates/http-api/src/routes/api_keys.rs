//! Browser-only workspace key lifecycle. Secrets leave only create/rotate,
//! with no-store. Machine access is decided in the shared PDP/PEP.
use crate::{
    auth::CurrentSession,
    error::{ApiError, ErrorResponse},
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use deepref_app::{
    api_keys::{self, ApiKeyError, KeySpec},
    identity::AuditRequest,
};
use deepref_domain::{ApiKeyId, WorkspaceId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Deserialize, ToSchema)]
pub(crate) struct CreateApiKeyRequest {
    name: String,
    /// Explicit read scopes: workspace:view and/or project:view.
    scopes: Vec<String>,
    expires_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct ApiKeyDto {
    id: Uuid,
    workspace_id: Uuid,
    created_by_user_id: Uuid,
    name: String,
    prefix: String,
    scopes: Vec<String>,
    expires_at: Option<DateTime<Utc>>,
    last_used_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

impl From<api_keys::ApiKeyRecord> for ApiKeyDto {
    fn from(key: api_keys::ApiKeyRecord) -> Self {
        Self {
            id: key.id,
            workspace_id: key.workspace_id,
            created_by_user_id: key.created_by_user_id,
            name: key.name,
            prefix: key.prefix,
            scopes: key.scopes,
            expires_at: key.expires_at,
            last_used_at: key.last_used_at,
            revoked_at: key.revoked_at,
            created_at: key.created_at,
        }
    }
}

// No Debug implementation: a response temporarily owns the exposed secret.
#[derive(Serialize, ToSchema)]
pub(crate) struct IssuedApiKeyDto {
    key: ApiKeyDto,
    /// Copy now. The full credential is never returned by another request.
    secret: String,
}

fn issued(key: api_keys::IssuedApiKey) -> Response {
    let body = IssuedApiKeyDto {
        key: key.record.into(),
        secret: key.secret.expose().to_owned(),
    };
    (
        StatusCode::CREATED,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::PRAGMA, "no-cache"),
        ],
        Json(body),
    )
        .into_response()
}

fn error(error: ApiKeyError) -> ApiError {
    match error {
        ApiKeyError::Denied
        | ApiKeyError::Authorization(deepref_application::authorization::AuthzError::Denied(_)) => {
            ApiError::NotFound("resource not found".into())
        }
        ApiKeyError::Invalid => {
            ApiError::BadRequest("invalid API key name, scopes or expiry".into())
        }
        ApiKeyError::Limit => ApiError::BadRequest("workspace API key limit reached".into()),
        ApiKeyError::Database(error) => error.into(),
        ApiKeyError::Random | ApiKeyError::Authorization(_) => {
            ApiError::Internal(anyhow::anyhow!("API key management is unavailable"))
        }
    }
}

#[utoipa::path(get, path = "/workspaces/{workspace_id}/api-keys", operation_id = "listApiKeys", tag = "api-keys",
    params(("workspace_id" = Uuid, Path)), responses((status = 200, body = [ApiKeyDto]), (status = 404, body = ErrorResponse)))]
pub(crate) async fn list(
    State(state): State<AppState>,
    Path(workspace): Path<Uuid>,
    CurrentSession(session): CurrentSession,
) -> Result<Json<Vec<ApiKeyDto>>, ApiError> {
    let actor = session
        .principal
        .user_id()
        .ok_or_else(|| ApiError::Forbidden("human session required".into()))?;
    Ok(Json(
        api_keys::list(&state.pool, actor, WorkspaceId::new(workspace))
            .await
            .map_err(error)?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

#[utoipa::path(post, path = "/workspaces/{workspace_id}/api-keys", operation_id = "createApiKey", tag = "api-keys", request_body = CreateApiKeyRequest,
    params(("workspace_id" = Uuid, Path)), responses((status = 201, body = IssuedApiKeyDto), (status = 400, body = ErrorResponse), (status = 404, body = ErrorResponse)))]
pub(crate) async fn create(
    State(state): State<AppState>,
    Path(workspace): Path<Uuid>,
    CurrentSession(session): CurrentSession,
    Json(input): Json<CreateApiKeyRequest>,
) -> Result<Response, ApiError> {
    let actor = session
        .principal
        .user_id()
        .ok_or_else(|| ApiError::Forbidden("human session required".into()))?;
    let spec =
        KeySpec::parse(input.name, input.scopes, input.expires_at, Utc::now()).map_err(error)?;
    let request = crate::error::correlation_id();
    let key = api_keys::create(
        &state.pool,
        actor,
        WorkspaceId::new(workspace),
        spec,
        AuditRequest {
            request_id: request.as_deref(),
            session_ref: None,
        },
    )
    .await
    .map_err(error)?;
    Ok(issued(key))
}

#[utoipa::path(delete, path = "/workspaces/{workspace_id}/api-keys/{key_id}", operation_id = "revokeApiKey", tag = "api-keys",
    params(("workspace_id" = Uuid, Path), ("key_id" = Uuid, Path)), responses((status = 204, description = "Key revoked"), (status = 404, body = ErrorResponse)))]
pub(crate) async fn revoke(
    State(state): State<AppState>,
    Path((workspace, key)): Path<(Uuid, Uuid)>,
    CurrentSession(session): CurrentSession,
) -> Result<StatusCode, ApiError> {
    let actor = session
        .principal
        .user_id()
        .ok_or_else(|| ApiError::Forbidden("human session required".into()))?;
    let request = crate::error::correlation_id();
    api_keys::revoke(
        &state.pool,
        actor,
        WorkspaceId::new(workspace),
        ApiKeyId::new(key),
        AuditRequest {
            request_id: request.as_deref(),
            session_ref: None,
        },
    )
    .await
    .map_err(error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/workspaces/{workspace_id}/api-keys/{key_id}/rotate", operation_id = "rotateApiKey", tag = "api-keys",
    params(("workspace_id" = Uuid, Path), ("key_id" = Uuid, Path)), responses((status = 201, body = IssuedApiKeyDto), (status = 404, body = ErrorResponse)))]
pub(crate) async fn rotate(
    State(state): State<AppState>,
    Path((workspace, key)): Path<(Uuid, Uuid)>,
    CurrentSession(session): CurrentSession,
) -> Result<Response, ApiError> {
    let actor = session
        .principal
        .user_id()
        .ok_or_else(|| ApiError::Forbidden("human session required".into()))?;
    let request = crate::error::correlation_id();
    Ok(issued(
        api_keys::rotate(
            &state.pool,
            actor,
            WorkspaceId::new(workspace),
            ApiKeyId::new(key),
            AuditRequest {
                request_id: request.as_deref(),
                session_ref: None,
            },
        )
        .await
        .map_err(error)?,
    ))
}
