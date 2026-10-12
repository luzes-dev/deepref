#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
//! DNS-rebinding defence: the API answers only requests addressed to a host it
//! knows. No database is needed; the pool is lazy and never connects.
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

mod support;

fn app() -> axum::Router {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://postgres:postgres@127.0.0.1:1/unused")
        .unwrap();
    let config = ApiConfig {
        runtime: support::local_runtime(),
        bind_addr: "127.0.0.1:8080".parse().unwrap(),
        cors_allow_any: false,
        cors_origins: vec![
            "http://localhost:5173".parse().unwrap(),
            "https://deepref.example.org".parse().unwrap(),
        ],
    };
    router(AppState::core(pool), &config)
}

async fn status_for(path: &str, host: Option<&str>) -> StatusCode {
    let mut request = Request::builder().uri(path);
    if let Some(host) = host {
        request = request.header(header::HOST, host);
    }
    app()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn foreign_host_is_rejected() {
    for host in [
        "evil.example",
        "evil.example:8080",
        "localhost.evil.example:8080",
        "127.0.0.1.evil.example",
    ] {
        assert_eq!(
            status_for("/openapi.json", Some(host)).await,
            StatusCode::FORBIDDEN,
            "{host} must be rejected"
        );
    }
}

#[tokio::test]
async fn loopback_and_configured_origin_hosts_are_accepted() {
    for host in [
        "localhost:8080",
        "LOCALHOST:8080",
        "127.0.0.1:8080",
        "[::1]:8080",
        "localhost",
        // A published container port or a same-origin proxy keeps the browser's port.
        "localhost:18080",
        "deepref.example.org",
        "deepref.example.org:443",
    ] {
        assert_eq!(
            status_for("/openapi.json", Some(host)).await,
            StatusCode::OK,
            "{host} must be accepted"
        );
    }
}

#[tokio::test]
async fn request_without_host_is_accepted() {
    // Browsers always send Host, so a rebinding page can never omit it; in-process
    // callers and HTTP/1.0 clients may.
    assert_eq!(status_for("/openapi.json", None).await, StatusCode::OK);
}

#[tokio::test]
async fn liveness_probe_ignores_host() {
    // Orchestrator probes address the pod or container IP.
    assert_eq!(
        status_for("/health/live", Some("10.1.2.3:8080")).await,
        StatusCode::OK
    );
}
