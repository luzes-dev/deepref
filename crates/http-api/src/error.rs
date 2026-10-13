use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

tokio::task_local! {
    pub(crate) static REQUEST_CORRELATION_ID: Uuid;
}

fn persistence_response(error: deepref_app::PersistenceError) -> (StatusCode, String, String, Option<u64>, Option<serde_json::Value>) {
    use deepref_app::PersistenceError;
    let (status, code, message, retry) = match error {
        PersistenceError::NotFound => (StatusCode::NOT_FOUND, "NOT_FOUND", "resource not found", None),
        PersistenceError::Conflict { .. } => (StatusCode::CONFLICT, "CONFLICT", "resource conflicts with existing data", None),
        PersistenceError::Integrity { .. } => (StatusCode::CONFLICT, "INTEGRITY_CONFLICT", "request conflicts with related data", None),
        PersistenceError::Retryable(_) => (StatusCode::SERVICE_UNAVAILABLE, "DATABASE_UNAVAILABLE", "database temporarily unavailable; retry later", Some(1)),
        PersistenceError::Other => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", "internal server error", None),
    };
    (status, code.to_owned(), message.to_owned(), retry, None)
}

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
    #[error("internal operation failed")]
    Internal(#[from] anyhow::Error),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    PayloadTooLarge(String),
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
    #[error("{0}")]
    Upstream(String),
    #[error("screening data integrity failure: {0}")]
    DataIntegrity(String),
    #[error("invalid JSON payload")]
    Json(#[from] serde_json::Error),
    #[error("invalid DOI: {0}")]
    Doi(#[from] deepref_domain::DoiError),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message, retry_after, details): (
            StatusCode,
            String,
            String,
            Option<u64>,
            Option<serde_json::Value>,
        ) = match self {
            Self::Database(sqlx::Error::RowNotFound) | Self::NotFound(_) => (
                StatusCode::NOT_FOUND,
                "NOT_FOUND".to_owned(),
                "resource not found".to_owned(),
                None,
                None,
            ),
            Self::Database(error) => {
                tracing::error!(%error, "database operation failed");
                persistence_response(deepref_app::classify(&error))
            }
            Self::Internal(error) => {
                tracing::error!(%error, "internal operation failed");
                use deepref_app::PersistenceFailure;
                if let Some(classified) = error.persistence_error() {
                    let (status, code, message, retry_after, details) = persistence_response(classified);
                    let body = ApiErrorBody { code, message, correlation_id: REQUEST_CORRELATION_ID.try_with(|id| *id).ok(), details };
                    let mut response = (status, Json(body)).into_response();
                    if retry_after.is_some() {
                        response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
                    }
                    return response;
                }
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
            Self::Upstream(message) => (
                StatusCode::BAD_GATEWAY,
                "UPSTREAM_UNAVAILABLE".to_owned(),
                message,
                None,
                None,
            ),
            Self::Forbidden(message) => (
                StatusCode::FORBIDDEN,
                "FORBIDDEN".to_owned(),
                message,
                None,
                None,
            ),
            Self::PayloadTooLarge(message) => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "PAYLOAD_TOO_LARGE".to_owned(),
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
            Self::DataIntegrity(message) => {
                tracing::error!(%message, "screening data integrity failure");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "INTERNAL_ERROR".to_owned(),
                    "internal server error".to_owned(),
                    None,
                    None,
                )
            }
            Self::Json(error) => (
                StatusCode::BAD_REQUEST,
                "INVALID_JSON".to_owned(),
                error.to_string(),
                None,
                None,
            ),
        };
        let body = ApiErrorBody {
            code,
            message,
            correlation_id: REQUEST_CORRELATION_ID.try_with(|id| *id).ok(),
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

#[cfg(test)]
mod tests {
    use axum::{body::to_bytes, response::IntoResponse};

    use super::*;

    #[tokio::test]
    async fn error_responses_always_include_a_correlation_id() {
        let id = Uuid::new_v4();
        let response = REQUEST_CORRELATION_ID.scope(id, async {
            ApiError::BadRequest("invalid request".to_owned()).into_response()
        }).await;
        let body: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("error body should be readable"),
        )
        .expect("error body should be JSON");
        let correlation_id = body["correlation_id"]
            .as_str()
            .and_then(|value| Uuid::parse_str(value).ok());
        assert_eq!(correlation_id, Some(id));
    }


    #[test]
    fn persistence_conflicts_and_integrity_are_http_conflicts() {
        use deepref_app::{IntegrityViolation, PersistenceError};
        for error in [
            PersistenceError::Conflict { constraint: Some("private_constraint".into()) },
            PersistenceError::Integrity { violation: IntegrityViolation::ForeignKey, constraint: None },
        ] {
            let (status, _, message, _, details) = persistence_response(error);
            assert_eq!(status, StatusCode::CONFLICT);
            assert!(!message.contains("private_constraint"));
            assert!(details.is_none());
        }
    }
}
