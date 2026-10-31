//! The object-store diagnostic stays bounded and secret-free when the backend
//! hangs or refuses connections. No database is needed: the PostgreSQL probe
//! runs against an unreachable address and reports unavailable on its own.
use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use serde_json::Value;
use tokio::net::TcpListener;
use tower::ServiceExt;

const SECRET: &str = "health-fixture-secret-never-echoed";
const ACCESS_KEY: &str = "health-fixture-access-key";

fn config() -> anyhow::Result<ApiConfig> {
    Ok(ApiConfig {
        runtime: deepref_config::RuntimeConfig::from_map(
            "health-object-store-fixture",
            &HashMap::from([("APP_ENV".to_owned(), "development".to_owned())]),
        )?,
        documents: deepref_config::DocumentConfig::from_source(&|key| {
            (key == "APP_ENV").then(|| "development".into())
        })?,
        bind_addr: "127.0.0.1:0".parse()?,
        cors_allow_any: false,
        cors_origins: vec![],
    })
}

fn s3_store(endpoint: &str) -> anyhow::Result<deepref_documents::DocumentStore> {
    let backend = object_store::aws::AmazonS3Builder::new()
        .with_endpoint(endpoint)
        .with_allow_http(true)
        .with_bucket_name("health-fixture")
        .with_region("us-east-1")
        .with_access_key_id(ACCESS_KEY)
        .with_secret_access_key(SECRET)
        .build()?;
    Ok(deepref_documents::DocumentStore::from_object_store(
        Arc::new(backend),
    ))
}

async fn dependencies(
    store: deepref_documents::DocumentStore,
) -> anyhow::Result<(Duration, Value)> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(200))
        .connect_lazy("postgres://deepref:deepref@127.0.0.1:1/deepref")?;
    let state = AppState::new(pool).with_document_store(store);
    let started = std::time::Instant::now();
    let response = router(state, &config()?)
        .oneshot(
            Request::builder()
                .uri("/health/dependencies")
                .body(Body::empty())?,
        )
        .await?;
    anyhow::ensure!(response.status() == 200);
    let body = to_bytes(response.into_body(), 1024 * 1024).await?;
    Ok((started.elapsed(), serde_json::from_slice(&body)?))
}

fn assert_secret_free(body: &Value, endpoint: &str) -> anyhow::Result<()> {
    let text = body.to_string();
    for needle in [SECRET, ACCESS_KEY, endpoint, "health-fixture"] {
        anyhow::ensure!(!text.contains(needle), "health leaked {needle}: {text}");
    }
    Ok(())
}

#[tokio::test]
async fn a_hanging_object_store_is_unavailable_within_the_probe_deadline() -> anyhow::Result<()> {
    // Accept connections and never answer: the slowest possible backend.
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = format!("http://{}", listener.local_addr()?);
    let hold = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    let (elapsed, body) = dependencies(s3_store(&endpoint)?).await?;
    hold.abort();
    anyhow::ensure!(
        body["object_storage"]["state"] == "unavailable",
        "a hung store must not look healthy: {body}"
    );
    anyhow::ensure!(body["object_storage"]["last_success_at"].is_null());
    anyhow::ensure!(
        elapsed < Duration::from_secs(10),
        "probe exceeded its bound: {elapsed:?}"
    );
    assert_secret_free(&body, &endpoint)
}

#[tokio::test]
async fn a_refusing_object_store_is_unavailable_and_secret_free() -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = format!("http://{}", listener.local_addr()?);
    drop(listener);
    let (elapsed, body) = dependencies(s3_store(&endpoint)?).await?;
    anyhow::ensure!(body["object_storage"]["state"] == "unavailable", "{body}");
    anyhow::ensure!(elapsed < Duration::from_secs(10), "{elapsed:?}");
    anyhow::ensure!(
        body["postgresql"]["state"] == "unavailable",
        "an unreachable database is unavailable, not stale evidence: {body}"
    );
    assert_secret_free(&body, &endpoint)
}

#[tokio::test]
async fn a_reachable_object_store_reports_fresh_success() -> anyhow::Result<()> {
    let (_, body) = dependencies(deepref_documents::DocumentStore::memory()).await?;
    anyhow::ensure!(body["object_storage"]["state"] == "available", "{body}");
    anyhow::ensure!(body["object_storage"]["last_success_at"].is_string());
    Ok(())
}
