#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use std::collections::HashMap;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode},
};
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;

async fn database() -> Option<PgPool> {
    let url = std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .expect("DATABASE_URL is set but PostgreSQL is unavailable");
    deepref_postgres::migrate(&pool)
        .await
        .expect("all migrations should apply");
    Some(pool)
}

fn api_config() -> ApiConfig {
    let runtime = deepref_config::RuntimeConfig::from_map(
        "deepref-api-settings-depth-test",
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

async fn call(pool: &PgPool, method: Method, uri: &str, body: Value) -> Response<Body> {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request should build");
    router(AppState::core(pool.clone()), &api_config())
        .oneshot(request)
        .await
        .expect("request should be handled")
}

async fn json_body(response: Response<Body>) -> Value {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should be readable");
    serde_json::from_slice(&body).expect("body should be JSON")
}

/// A project created without a depth takes the workspace Settings default, and an explicit depth
/// still wins. The Imports page sends no depth for a new project, so this is the value it inherits.
#[tokio::test]
async fn projects_created_without_a_depth_take_the_settings_default() {
    let Some(pool) = database().await else {
        return;
    };

    let response = call(
        &pool,
        Method::PATCH,
        "/settings",
        json!({ "default_max_depth": 0 }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["default_max_depth"], 0);

    let inherited = call(
        &pool,
        Method::POST,
        "/projects",
        json!({ "name": "settings default depth" }),
    )
    .await;
    assert_eq!(inherited.status(), StatusCode::CREATED);
    assert_eq!(json_body(inherited).await["default_max_depth"], 0);

    let explicit = call(
        &pool,
        Method::POST,
        "/projects",
        json!({ "name": "explicit depth", "default_max_depth": 3 }),
    )
    .await;
    assert_eq!(explicit.status(), StatusCode::CREATED);
    assert_eq!(json_body(explicit).await["default_max_depth"], 3);

    let restored = call(
        &pool,
        Method::PATCH,
        "/settings",
        json!({ "default_max_depth": 1 }),
    )
    .await;
    assert_eq!(restored.status(), StatusCode::OK);
}
