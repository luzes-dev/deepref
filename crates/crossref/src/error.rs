use reqwest::StatusCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CrossrefError {
    #[error("Crossref mailto setting is required")]
    MissingMailto,
    #[error("Crossref returned not found for {0}")]
    NotFound(String),
    #[error("Crossref blocked or rejected the request: {0}")]
    Blocked(StatusCode),
    #[error("Crossref returned retryable status: {0}")]
    RetryableStatus(StatusCode),
    #[error("Crossref returned non-retryable status: {0}")]
    NonRetryableStatus(StatusCode),
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("invalid Crossref response: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid DOI: {0}")]
    InvalidDoi(#[from] deepref_core::DoiError),
}
