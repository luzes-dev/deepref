#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
//! `/health/dependencies` reports worker liveness from `worker_heartbeats`.
use std::collections::HashMap;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use deepref_domain::WorkloadClass;
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let pool = deepref_app::test_isolation::isolated_pool_from_env_first(
        &["TEST_DATABASE_URL", "DATABASE_URL"],
        5,
    )
    .await?;
    deepref_app::migrate(&pool)
        .await
        .expect("all migrations should apply");
    Some(pool)
}

fn api_config() -> ApiConfig {
    let runtime = deepref_config::RuntimeConfig::from_map(
        "deepref-api-health-worker-test",
        &HashMap::from([("APP_ENV".to_owned(), "development".to_owned())]),
    )
    .expect("local test runtime should parse");
    ApiConfig {
        documents: deepref_config::DocumentConfig::from_source(&|name| {
            (name == "APP_ENV").then(|| "development".into())
        })
        .expect("local test document configuration"),
        runtime,
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        cors_allow_any: false,
        cors_origins: vec!["http://localhost:3000".parse().unwrap()],
    }
}

async fn worker_health(pool: &PgPool) -> Value {
    let request = Request::builder()
        .uri("/health/dependencies")
        .body(Body::empty())
        .expect("request should build");
    let response = router(AppState::core(pool.clone()), &api_config())
        .oneshot(request)
        .await
        .expect("request should be handled");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should be readable");
    let body: Value = serde_json::from_slice(&body).expect("body should be JSON");
    body["worker"].clone()
}

#[tokio::test]
async fn dependencies_report_per_pool_queue_age_inflight_and_capacity() {
    let Some(pool) = database().await else { return };

    // No worker has ever run: none is live, and queued work is not progressing.
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'health worker fixture')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("project");
    sqlx::query(
        "INSERT INTO jobs (id,project_id,kind,workload_class,payload,dedupe_key)
         VALUES ($1,$2,'health_fixture','workflow','{}',$3)",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(format!("health-fixture:{project_id}"))
    .execute(&pool)
    .await
    .expect("queued job");
    sqlx::query(
        "INSERT INTO jobs (id,project_id,kind,workload_class,state,lease_owner,leased_until,payload,dedupe_key)
         VALUES ($1,$2,'health_running','workflow','running','health-worker',now()+interval '1 minute','{}',$3)",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(format!("health-running:{project_id}"))
    .execute(&pool)
    .await
    .expect("running job");
    sqlx::query(
        "INSERT INTO jobs (id,project_id,kind,workload_class,state,available_at,payload,dedupe_key)
         VALUES ($1,$2,'health_future','workflow','queued',now()+interval '1 hour','{}',$3)",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(format!("health-future:{project_id}"))
    .execute(&pool)
    .await
    .expect("future job");
    let worker = worker_health(&pool).await;
    assert_eq!(worker["live_workers"], 0, "{worker}");
    assert!(worker["last_heartbeat_at"].is_null(), "{worker}");
    assert_eq!(
        worker["state"], "degraded",
        "queued work with no live worker is degraded: {worker}"
    );
    let workflow = worker["pools"]
        .as_array()
        .expect("workload pool measurements")
        .iter()
        .find(|pool| pool["class"] == "workflow")
        .expect("workflow pool is reported");
    assert_eq!(
        workflow["queue_depth"], 1,
        "future work is not due: {worker}"
    );
    assert!(
        workflow["oldest_due_age_seconds"].as_i64().is_some(),
        "{worker}"
    );
    assert_eq!(workflow["in_flight"], 1, "{worker}");
    assert_eq!(
        workflow["capacity"], 0,
        "no worker capacity is live: {worker}"
    );
    assert!(workflow["saturation"].is_null(), "{worker}");

    // A worker heartbeats: it is live and reported.
    deepref_app::record_worker_heartbeat_with_capacities(
        &pool,
        "health-test-worker",
        &[(WorkloadClass::Workflow, 2), (WorkloadClass::Documents, 4)],
    )
    .await
    .expect("heartbeat");
    let worker = worker_health(&pool).await;
    assert_eq!(worker["live_workers"], 1, "{worker}");
    assert!(worker["last_heartbeat_at"].is_string(), "{worker}");
    assert_eq!(worker["state"], "available", "{worker}");
    let workflow = worker["pools"]
        .as_array()
        .expect("workload pool measurements")
        .iter()
        .find(|pool| pool["class"] == "workflow")
        .expect("workflow pool is reported");
    assert_eq!(workflow["queue_depth"], 1, "{worker}");
    assert_eq!(workflow["in_flight"], 1, "{worker}");
    assert_eq!(workflow["capacity"], 2, "{worker}");
    assert_eq!(workflow["saturation"], 0.5, "{worker}");

    // Its heartbeat goes stale (the process died without a clean shutdown).
    sqlx::query("UPDATE worker_heartbeats SET seen_at = now() - interval '10 minutes'")
        .execute(&pool)
        .await
        .expect("age heartbeat");
    let worker = worker_health(&pool).await;
    assert_eq!(worker["live_workers"], 0, "{worker}");
    assert!(worker["last_heartbeat_at"].is_string(), "{worker}");
    assert_eq!(worker["state"], "degraded", "{worker}");
}
