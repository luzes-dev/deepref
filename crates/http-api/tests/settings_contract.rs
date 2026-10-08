#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use std::collections::HashMap;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode},
};
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

fn api_config() -> ApiConfig {
    let runtime = deepref_config::RuntimeConfig::from_map(
        "deepref-api-settings-test",
        &HashMap::from([("APP_ENV".to_owned(), "local".to_owned())]),
    )
    .expect("local test runtime should parse");
    ApiConfig {
        runtime,
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        cors_allow_any: false,
        cors_origins: vec!["http://localhost:3000".parse().unwrap()],
    }
}

/// Validation runs before any query, so the pool never connects and this test
/// needs no PostgreSQL instance.
#[tokio::test]
async fn patch_settings_rejects_a_malformed_crossref_mailto_with_400() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://postgres:postgres@127.0.0.1:1/unused")
        .expect("lazy pool should be constructed");
    let request = Request::builder()
        .method(Method::PATCH)
        .uri("/settings")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "crossref_mailto": "not-an-email" }).to_string(),
        ))
        .expect("request should build");

    let response = router(AppState::core(pool), &api_config())
        .oneshot(request)
        .await
        .expect("request should be handled");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should be readable");
    let payload: Value = serde_json::from_slice(&body).expect("body should be JSON");
    assert!(
        payload.to_string().contains("crossref_mailto"),
        "the error should name the field: {payload}"
    );
}
