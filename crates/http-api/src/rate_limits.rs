use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use deepref_application::rate_limits::{RateLimitDecision, RateLimitPolicy, RateLimitSubject};

use crate::state::AppState;

/// Returns a refusal response, or lets the request continue to its ordinary
/// authorization checks. A limiter grant never grants access to a resource.
pub(crate) async fn refusal(
    state: &AppState,
    subject: RateLimitSubject,
    policy: RateLimitPolicy,
) -> Option<Response> {
    match state.rate_limiter.check(subject, policy).await {
        Ok(RateLimitDecision::Allowed) => None,
        Ok(RateLimitDecision::Limited) => Some((
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, "60")],
            Json(serde_json::json!({"code": "RATE_LIMITED", "message": "Too many calls. Try again in a minute."})),
        ).into_response()),
        Err(_) => Some((
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::RETRY_AFTER, "1")],
            Json(serde_json::json!({"code": "RATE_LIMIT_UNAVAILABLE", "message": "Shared rate limiting is temporarily unavailable."})),
        ).into_response()),
    }
}
