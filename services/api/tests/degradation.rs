use axum::{
    http::{StatusCode, header},
    response::IntoResponse,
};

#[test]
fn graph_unavailable_is_typed_and_retryable() {
    let response =
        deepref_api::error::ApiError::graph_unavailable(std::time::Duration::from_secs(17))
            .into_response();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[header::RETRY_AFTER], "17");
}
