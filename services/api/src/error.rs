use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ApiErrorBody {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("{0}")]
    BadRequest(String),
    #[error("{message}")]
    Conflict {
        code: String,
        message: String,
        details: serde_json::Value,
    },
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Configuration(String),
    #[error("graph read model is unavailable")]
    GraphUnavailable { retry_after_seconds: u64 },
    #[error("invalid JSON payload")]
    Json(#[from] serde_json::Error),
    #[error("invalid DOI: {0}")]
    Doi(#[from] deepref_core::DoiError),
}

impl ApiError {
    pub fn graph_unavailable(retry_after: std::time::Duration) -> Self {
        Self::GraphUnavailable {
            retry_after_seconds: retry_after.as_secs().max(1),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let correlation_id = None;
        let (status, code, message, retry_after, details) = match self {
            Self::Database(sqlx::Error::RowNotFound) | Self::NotFound(_) => (
                StatusCode::NOT_FOUND,
                "NOT_FOUND".to_owned(),
                "resource not found".to_owned(),
                None,
                None,
            ),
            Self::Database(error) => {
                tracing::error!(%error, "database operation failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "INTERNAL_ERROR".to_owned(),
                    "internal server error".to_owned(),
                    None,
                    None,
                )
            }
            Self::BadRequest(message) => (
                StatusCode::BAD_REQUEST,
                "INVALID_REQUEST".to_owned(),
                message,
                None,
                None,
            ),
            Self::Conflict {
                code,
                message,
                details,
            } => (StatusCode::CONFLICT, code, message, None, Some(details)),
            Self::Doi(error) => (
                StatusCode::BAD_REQUEST,
                "INVALID_DOI".to_owned(),
                error.to_string(),
                None,
                None,
            ),
            Self::Configuration(message) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "CONFIGURATION_ERROR".to_owned(),
                message,
                None,
                None,
            ),
            Self::GraphUnavailable {
                retry_after_seconds,
            } => (
                StatusCode::SERVICE_UNAVAILABLE,
                "GRAPH_UNAVAILABLE".to_owned(),
                "graph features are temporarily unavailable".to_owned(),
                Some(retry_after_seconds),
                None,
            ),
            Self::Json(error) => (
                StatusCode::BAD_REQUEST,
                "INVALID_JSON".to_owned(),
                error.to_string(),
                None,
                None,
            ),
        };
        let body = ApiErrorBody {
            code: code.to_owned(),
            message,
            correlation_id,
            details,
        };
        let mut response = (status, Json(body)).into_response();
        if let Some(seconds) = retry_after
            && let Ok(value) = HeaderValue::from_str(&seconds.to_string())
        {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        response
    }
}

pub type ErrorResponse = ApiErrorBody;
