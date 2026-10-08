#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
use std::collections::HashMap;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
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
        "deepref-api-extraction-manual-test",
        &HashMap::from([("APP_ENV".to_owned(), "local".to_owned())]),
    )
    .expect("local test runtime should parse");
    ApiConfig {
        runtime,
        bind_addr: "127.0.0.1:0".parse().expect("bind address"),
        cors_allow_any: false,
        cors_origins: Vec::new(),
    }
}

async fn call(
    pool: &PgPool,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-actor-id", "reviewer-http");
    let body = match body {
        Some(json) => {
            builder = builder.header("content-type", "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let response = router(AppState::core(pool.clone()), &api_config())
        .oneshot(builder.body(body).expect("request"))
        .await
        .expect("handled");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

#[tokio::test]
async fn reviewer_can_record_overwrite_and_clear_a_value_without_ai() {
    let Some(pool) = database().await else { return };
    let project_id = Uuid::new_v4();
    let study_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'manual extraction http')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("project");
    sqlx::query(
        "INSERT INTO studies
         (id,project_id,title,design_context,study_revision,updated_by_actor_kind,updated_by_actor_id)
         VALUES ($1,$2,'Manual study','{}'::jsonb,0,'system','test')",
    )
    .bind(study_id)
    .bind(project_id)
    .execute(&pool)
    .await
    .expect("study");

    let (status, field) = call(
        &pool,
        "POST",
        &format!("/projects/{project_id}/extraction/fields"),
        Some(serde_json::json!({
            "version": 1, "field_key": "n", "label": "Sample size",
            "value_type": "number", "required": false
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let field_id = field["id"].as_str().expect("field id").to_owned();
    let uri = format!("/projects/{project_id}/studies/{study_id}/extraction/values/{field_id}");

    let (status, saved) = call(
        &pool,
        "PUT",
        &uri,
        Some(serde_json::json!({"value": {"kind": "number", "value": 12.0}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["approved_by_actor_id"], "reviewer-http");
    assert!(saved["source_block_id"].is_null());

    let (status, _) = call(
        &pool,
        "PUT",
        &uri,
        Some(serde_json::json!({"value": {"kind": "text", "value": "x"}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = call(
        &pool,
        "PUT",
        &uri,
        Some(serde_json::json!({"value": {"kind": "number", "value": 15.0}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let list_uri = format!("/projects/{project_id}/studies/{study_id}/extraction");
    let (_, listed) = call(&pool, "GET", &list_uri, None).await;
    assert_eq!(listed.as_array().expect("list").len(), 1);
    assert_eq!(listed[0]["value"]["value"], 15.0);

    let (status, _) = call(&pool, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, listed) = call(&pool, "GET", &list_uri, None).await;
    assert!(listed.as_array().expect("list").is_empty());

    let missing = format!(
        "/projects/{project_id}/studies/{}/extraction/values/{field_id}",
        Uuid::new_v4()
    );
    let (status, _) = call(
        &pool,
        "PUT",
        &missing,
        Some(serde_json::json!({"value": {"kind": "number", "value": 1.0}})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
