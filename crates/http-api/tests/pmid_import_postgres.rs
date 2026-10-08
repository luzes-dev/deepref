#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use std::collections::HashMap;

use axum::{
    body::{Body, to_bytes},
    http::{Request, Response, StatusCode},
};
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use serde_json::json;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

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
        "deepref-api-pmid-import-test",
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

async fn call(pool: &PgPool, request: Request<Body>) -> Response<Body> {
    router(AppState::core(pool.clone()), &api_config())
        .oneshot(request)
        .await
        .expect("request should be handled")
}

async fn json_body(response: Response<Body>) -> serde_json::Value {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should be readable");
    serde_json::from_slice(&body).expect("body should be JSON")
}

async fn create_project(pool: &PgPool) -> Uuid {
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'pmid import api test')")
        .bind(project_id)
        .execute(pool)
        .await
        .expect("project should be created");
    project_id
}

fn import_request(project_id: Uuid, key: Option<&str>, content: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/imports"))
        .header("content-type", "application/json");
    if let Some(key) = key {
        builder = builder.header("idempotency-key", key);
    }
    builder
        .body(Body::from(
            json!({"format": "pmid", "content": content}).to_string(),
        ))
        .expect("import request should be valid")
}

#[tokio::test]
async fn a_pmid_import_is_a_queued_run_with_one_item_per_id_and_one_job() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = create_project(&pool).await;
    let content = "PMID: 19446324\n123, 19446324\nhttps://pubmed.ncbi.nlm.nih.gov/456/";

    let created = call(
        &pool,
        import_request(project_id, Some("pmid-api-1"), content),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let run = json_body(created).await;
    assert_eq!(run["strategy"], "pmid_import");
    assert_eq!(run["format"], "pmid");
    assert_eq!(run["status"], "queued");
    assert_eq!(run["seed_count"], 3, "the repeated PMID is kept once");
    assert_eq!(run["queued_count"], 3);
    let run_id: Uuid = run["id"].as_str().unwrap().parse().unwrap();

    let jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE project_id=$1 AND kind='pmid_import' AND payload->>'acquisition_id'=$2",
    )
    .bind(project_id)
    .bind(run_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(jobs, 1, "the worker is asked once to fetch the run");

    let replay = call(
        &pool,
        import_request(project_id, Some("pmid-api-1"), content),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(json_body(replay).await["id"], run["id"]);

    let reused = call(
        &pool,
        import_request(project_id, Some("pmid-api-1"), "999999999"),
    )
    .await;
    assert_eq!(reused.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(reused).await["code"], "IDEMPOTENCY_KEY_REUSED");

    let listed = call(
        &pool,
        Request::builder()
            .uri(format!(
                "/projects/{project_id}/acquisitions?strategy=pmid_import"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = json_body(listed).await;
    assert_eq!(listed["items"][0]["id"], run["id"]);
}

#[tokio::test]
async fn a_pmid_import_pages_its_items_in_the_order_given() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = create_project(&pool).await;
    let created = call(
        &pool,
        import_request(project_id, None, "19446324\n123\n456"),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let run_id = json_body(created).await["id"].as_str().unwrap().to_owned();
    let items_uri = format!("/projects/{project_id}/acquisitions/{run_id}/items");

    let first = json_body(
        call(
            &pool,
            Request::builder()
                .uri(format!("{items_uri}?limit=2"))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    let identifiers: Vec<&str> = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["identifier"].as_str().unwrap())
        .collect();
    assert_eq!(identifiers, ["19446324", "123"]);
    assert_eq!(first["items"][0]["status"], "queued");
    let cursor = first["next_cursor"].as_str().expect("a second page exists");

    let second = json_body(
        call(
            &pool,
            Request::builder()
                .uri(format!("{items_uri}?limit=2&cursor={cursor}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(second["items"][0]["identifier"], "456");
    assert!(second["next_cursor"].is_null());
}

#[tokio::test]
async fn a_pmid_import_items_are_only_visible_inside_their_project() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = create_project(&pool).await;
    let other_project = create_project(&pool).await;
    let created = call(&pool, import_request(project_id, None, "19446324")).await;
    let run_id = json_body(created).await["id"].as_str().unwrap().to_owned();

    let response = call(
        &pool,
        Request::builder()
            .uri(format!(
                "/projects/{other_project}/acquisitions/{run_id}/items"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let run_only_here = call(
        &pool,
        Request::builder()
            .uri(format!("/projects/{project_id}/acquisitions/{run_id}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(run_only_here.status(), StatusCode::OK);
    assert_eq!(json_body(run_only_here).await["strategy"], "pmid_import");
    let run_elsewhere = call(
        &pool,
        Request::builder()
            .uri(format!("/projects/{other_project}/acquisitions/{run_id}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(run_elsewhere.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_pmid_import_rejects_text_that_is_not_a_list_of_pubmed_ids() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = create_project(&pool).await;
    let rejected = call(
        &pool,
        import_request(project_id, None, "19446324\nnot-a-pmid"),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    let message = json_body(rejected).await;
    assert!(
        message.to_string().contains("not-a-pmid"),
        "the response names the bad line: {message}"
    );
    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM acquisition_runs WHERE project_id=$1")
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(runs, 0, "nothing is created for a rejected list");
}
