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
            "http://localhost:4173".parse().unwrap(),
            "http://127.0.0.1:4173".parse().unwrap(),
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
        "localhost:18080",
        "deepref.example.org:8080",
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
        "localhost:5173",
        "localhost:4173",
        "127.0.0.1:4173",
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
    // In-process and HTTP/1.0 callers can omit Host; browsers cannot.
    assert_eq!(status_for("/openapi.json", None).await, StatusCode::OK);
}

#[tokio::test]
async fn liveness_probe_rejects_unknown_host() {
    assert_eq!(
        status_for("/health/live", Some("10.1.2.3:8080")).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status_for("/health/live", Some("127.0.0.1:8080")).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn client_automation_and_system_actors_are_rejected_before_database_access() {
    for kind in ["automation", "system"] {
        let request = Request::builder()
            .uri("/projects/00000000-0000-0000-0000-000000000001/screening")
            .header(header::HOST, "localhost:8080")
            .header("x-actor-kind", kind)
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app().oneshot(request).await.unwrap().status(),
            StatusCode::BAD_REQUEST,
            "{kind} must be rejected"
        );
    }
}
