use reqwest::StatusCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusClassification {
    Retryable,
    NonRetryable,
    NotFound,
    Blocked,
}

pub fn classify_status(status: StatusCode) -> StatusClassification {
    match status {
        StatusCode::NOT_FOUND => StatusClassification::NotFound,
        StatusCode::FORBIDDEN => StatusClassification::Blocked,
        StatusCode::REQUEST_TIMEOUT
        | StatusCode::TOO_MANY_REQUESTS
        | StatusCode::INTERNAL_SERVER_ERROR
        | StatusCode::BAD_GATEWAY
        | StatusCode::SERVICE_UNAVAILABLE
        | StatusCode::GATEWAY_TIMEOUT => StatusClassification::Retryable,
        _ if status.is_client_error() => StatusClassification::NonRetryable,
        _ if status.is_server_error() => StatusClassification::Retryable,
        _ => StatusClassification::NonRetryable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_retryable_statuses() {
        assert_eq!(
            classify_status(StatusCode::TOO_MANY_REQUESTS),
            StatusClassification::Retryable
        );
        assert_eq!(
            classify_status(StatusCode::FORBIDDEN),
            StatusClassification::Blocked
        );
        assert_eq!(
            classify_status(StatusCode::NOT_FOUND),
            StatusClassification::NotFound
        );
    }
}
