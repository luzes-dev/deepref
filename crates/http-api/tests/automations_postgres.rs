#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, response::Response},
};
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use serde_json::Value;
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::collections::HashMap;
use std::sync::OnceLock;
use tower::ServiceExt;
use uuid::Uuid;

static DATABASE_TEST_MUTEX: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn test_lock() -> &'static tokio::sync::Mutex<()> {
    DATABASE_TEST_MUTEX.get_or_init(tokio::sync::Mutex::default)
}

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .expect("DATABASE_URL database must be reachable");
    deepref_postgres::migrate(&pool)
        .await
        .expect("DATABASE_URL migrations must apply");
    Some(pool)
}

fn api_config() -> ApiConfig {
    let runtime = deepref_config::RuntimeConfig::from_map(
        "deepref-api-automations-test",
        &HashMap::from([("APP_ENV".to_owned(), "local".to_owned())]),
    )
    .expect("local test runtime should parse");
    ApiConfig {
        runtime,
        bind_addr: "127.0.0.1:0".parse().expect("test bind address is valid"),
        cors_allow_any: false,
        cors_origins: Vec::new(),
    }
}

async fn response_json(response: Response<Body>) -> Value {
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    serde_json::from_slice(&body).unwrap_or_else(|error| {
        panic!(
            "response should be JSON: {error}; status={status}; headers={headers:?}; body={:?}",
            String::from_utf8_lossy(&body)
        )
    })
}

async fn cleanup(pool: &PgPool, project_ids: &[Uuid]) {
    sqlx::query("DELETE FROM automation_step_runs WHERE project_id = ANY($1)")
        .bind(project_ids)
        .execute(pool)
        .await
        .expect("automation step-run fixtures should clean up");
    sqlx::query("DELETE FROM automation_runs WHERE project_id = ANY($1)")
        .bind(project_ids)
        .execute(pool)
        .await
        .expect("automation run fixtures should clean up");
    sqlx::query("DELETE FROM jobs WHERE project_id = ANY($1) AND kind = 'automation_run'")
        .bind(project_ids)
        .execute(pool)
        .await
        .expect("automation job fixtures should clean up");
    sqlx::query("DELETE FROM automation_definition_steps WHERE project_id = ANY($1)")
        .bind(project_ids)
        .execute(pool)
        .await
        .expect("automation definition-step fixtures should clean up");
    sqlx::query("DELETE FROM automation_definitions WHERE project_id = ANY($1)")
        .bind(project_ids)
        .execute(pool)
        .await
        .expect("automation definition fixtures should clean up");
    sqlx::query("DELETE FROM projects WHERE id = ANY($1)")
        .bind(project_ids)
        .execute(pool)
        .await
        .expect("automation project fixtures should clean up");
}

async fn project(pool: &PgPool, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,$2)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await
        .expect("project fixture should insert");
    id
}

async fn request(
    pool: &PgPool,
    method: &str,
    uri: String,
    body: Value,
    idempotency_key: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-actor-kind", "user")
        .header("x-actor-id", "automations-http-test");
    if let Some(key) = idempotency_key {
        builder = builder.header("idempotency-key", key);
    }
    let response = router(AppState::core(pool.clone()), &api_config())
        .oneshot(
            builder
                .body(Body::from(body.to_string()))
                .expect("automation request should be valid"),
        )
        .await
        .expect("automation request should be handled");
    let status = response.status();
    (status, response_json(response).await)
}

#[tokio::test]
async fn automation_http_api_configures_lists_and_manually_starts_idempotently() {
    let _guard = test_lock().lock().await;
    let Some(pool) = database().await else {
        return;
    };
    let project_id = project(&pool, "automation HTTP project").await;
    let other_project_id = project(&pool, "automation HTTP isolation project").await;
    let result = async {
        let (status, definition) = request(
            &pool,
            "PUT",
            format!("/projects/{project_id}/automations/definitions/project_maintenance.v1"),
            serde_json::json!({
                "name": "Project maintenance",
                "trigger": "manual",
                "status": "active"
            }),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(definition["recipe"], "project_maintenance");
        assert_eq!(definition["version"], 1);
        assert_eq!(definition["trigger"], "manual");
        assert_eq!(definition["status"], "active");
        assert_eq!(definition["steps"][0]["key"], "recompute_project_metrics");

        let definition_id = definition["id"]
            .as_str()
            .and_then(|id| Uuid::parse_str(id).ok())
            .expect("definition id should be a UUID");
        let (status, definitions) = request(
            &pool,
            "GET",
            format!("/projects/{project_id}/automations/definitions"),
            Value::Null,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(definitions.as_array().map(Vec::len), Some(1));

        let (status, other_definitions) = request(
            &pool,
            "GET",
            format!("/projects/{other_project_id}/automations/definitions"),
            Value::Null,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(other_definitions.as_array().map(Vec::len), Some(0));

        let (status, missing_key) = request(
            &pool,
            "POST",
            format!("/projects/{project_id}/automations/runs"),
            serde_json::json!({ "definition_id": definition_id }),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(missing_key["code"], "INVALID_REQUEST");

        let (status, started) = request(
            &pool,
            "POST",
            format!("/projects/{project_id}/automations/runs"),
            serde_json::json!({ "definition_id": definition_id }),
            Some("automation-http-1"),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(started["created"], true);
        let run_id = started["run_id"]
            .as_str()
            .and_then(|id| Uuid::parse_str(id).ok())
            .expect("started run id should be a UUID");

        let (status, replay) = request(
            &pool,
            "POST",
            format!("/projects/{project_id}/automations/runs"),
            serde_json::json!({ "definition_id": definition_id }),
            Some("automation-http-1"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(replay["created"], false);
        assert_eq!(replay["run_id"], run_id.to_string());

        let (status, runs) = request(
            &pool,
            "GET",
            format!("/projects/{project_id}/automations/runs?limit=1"),
            Value::Null,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(runs.as_array().map(Vec::len), Some(1));
        assert_eq!(runs[0]["steps"][0]["status"], "pending");
        assert!(runs[0]["job"]["status"].is_string());
        assert!(runs[0]["usage"]["cost_micros"].is_number());

        let (status, run) = request(
            &pool,
            "GET",
            format!("/projects/{project_id}/automations/runs/{run_id}"),
            Value::Null,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(run["project_id"], project_id.to_string());

        let (status, isolated_run) = request(
            &pool,
            "GET",
            format!("/projects/{other_project_id}/automations/runs/{run_id}"),
            Value::Null,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(isolated_run["code"], "NOT_FOUND");
        Ok::<(), anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &[project_id, other_project_id]).await;
    result.expect("automation HTTP lifecycle should pass");
}

#[tokio::test]
async fn automation_http_api_rejects_unknown_recipe_and_invalid_limits() {
    let _guard = test_lock().lock().await;
    let Some(pool) = database().await else {
        return;
    };
    let project_id = project(&pool, "automation HTTP validation project").await;
    let result = async {
        let (status, unknown_recipe) = request(
            &pool,
            "PUT",
            format!("/projects/{project_id}/automations/definitions/not_a_recipe"),
            serde_json::json!({
                "name": "Unsupported",
                "trigger": "manual",
                "status": "active"
            }),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(unknown_recipe["code"], "INVALID_REQUEST");

        let (status, invalid_limit) = request(
            &pool,
            "GET",
            format!("/projects/{project_id}/automations/runs?limit=101"),
            Value::Null,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(invalid_limit["code"], "INVALID_REQUEST");

        let (status, missing_definition) = request(
            &pool,
            "POST",
            format!("/projects/{project_id}/automations/runs"),
            serde_json::json!({ "definition_id": Uuid::new_v4() }),
            Some("automation-http-missing-definition"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(missing_definition["code"], "NOT_FOUND");

        let (status, paused_definition) = request(
            &pool,
            "PUT",
            format!("/projects/{project_id}/automations/definitions/project_maintenance.v1"),
            serde_json::json!({
                "name": "Paused project maintenance",
                "trigger": "manual",
                "status": "paused"
            }),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(paused_definition["status"], "paused");
        let paused_definition_id = paused_definition["id"]
            .as_str()
            .and_then(|id| Uuid::parse_str(id).ok())
            .expect("paused definition id should be a UUID");

        let (status, paused_start) = request(
            &pool,
            "POST",
            format!("/projects/{project_id}/automations/runs"),
            serde_json::json!({ "definition_id": paused_definition_id }),
            Some("automation-http-paused"),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(paused_start["code"], "AUTOMATION_PAUSED");
        Ok::<(), anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &[project_id]).await;
    result.expect("automation validation should pass");
}
