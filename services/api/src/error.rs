use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ErrorResponse {
    pub(crate) error: String,
}

#[derive(Debug)]
pub(crate) enum ApiError {
    Db(sqlx::Error),
    BadRequest(String),
    Json(serde_json::Error),
    Doi(deepref_core::DoiError),
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error)
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<deepref_core::DoiError> for ApiError {
    fn from(error: deepref_core::DoiError) -> Self {
        Self::Doi(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::Db(sqlx::Error::RowNotFound) => {
                (StatusCode::NOT_FOUND, "not found".to_owned())
            }
            ApiError::Db(error) => {
                tracing::error!(%error, "database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_owned(),
                )
            }
            ApiError::BadRequest(error) => (StatusCode::BAD_REQUEST, error),
            ApiError::Json(error) => (StatusCode::BAD_REQUEST, error.to_string()),
            ApiError::Doi(error) => (StatusCode::BAD_REQUEST, error.to_string()),
        };
        (status, Json(ErrorResponse { error: message })).into_response()
    }
}
