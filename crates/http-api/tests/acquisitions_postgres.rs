use std::collections::HashMap;

use axum::{
    body::{Body, to_bytes},
    http::{Request, Response, StatusCode},
};
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use serde_json::json;
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
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
        "deepref-api-acquisition-test",
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

#[tokio::test]
async fn imports_are_idempotent_and_preserve_raw_records_without_reports() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'acquisition import test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();

    let payload = json!({
        "format": "ris",
        "content": "TY  - JOUR\nTI  - Über die Wirkung von Kaffee\nAU  - Müller, Ana\nPY  - 2024/\nJO  - Journal of Café Studies\nDO  - 10.5555/example-unicode\nER  -\n"
    });
    let request = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/imports"))
        .header("content-type", "application/json")
        .header("idempotency-key", "import-test-1")
        .body(Body::from(payload.to_string()))
        .unwrap();
    let first = call(&pool, request).await;
    assert_eq!(first.status(), StatusCode::CREATED);
    let first = json_body(first).await;
    let run_id = first["id"].as_str().unwrap().parse::<Uuid>().unwrap();

    let record_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM records WHERE project_id=$1 AND acquisition_run_id=$2",
    )
    .bind(project_id)
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(record_count, 1);
    let record = sqlx::query(
        "SELECT report_id,source,source_key,journal,authors,source_identifiers
         FROM records WHERE project_id=$1 AND acquisition_run_id=$2",
    )
    .bind(project_id)
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(record.get::<Option<Uuid>, _>("report_id").is_none());
    assert_eq!(record.get::<String, _>("source"), "import:ris");
    assert!(
        record
            .get::<String, _>("source_key")
            .starts_with(&run_id.to_string())
    );
    assert_eq!(
        record.get::<String, _>("journal"),
        "Journal of Café Studies"
    );
    assert_eq!(
        record.get::<serde_json::Value, _>("source_identifiers")[0]["normalized_value"],
        "10.5555/example-unicode"
    );
    assert_eq!(
        record.get::<serde_json::Value, _>("authors")[0]["family"],
        "Müller"
    );
    let identifier_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM record_identifiers WHERE record_id IN (SELECT id FROM records WHERE acquisition_run_id=$1)",
    )
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(identifier_count, 1);

    let repeat = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/imports"))
        .header("content-type", "application/json")
        .header("idempotency-key", "import-test-1")
        .body(Body::from(payload.to_string()))
        .unwrap();
    let repeat = call(&pool, repeat).await;
    assert_eq!(repeat.status(), StatusCode::OK);
    assert_eq!(json_body(repeat).await["id"], run_id.to_string());
    let repeated_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM records WHERE acquisition_run_id=$1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(repeated_count, 1);

    let conflict_payload = json!({
        "format": "doi",
        "content": "10.5555/different"
    });
    let conflict = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/imports"))
        .header("content-type", "application/json")
        .header("idempotency-key", "import-test-1")
        .body(Body::from(conflict_payload.to_string()))
        .unwrap();
    assert_eq!(call(&pool, conflict).await.status(), StatusCode::CONFLICT);

    let csv_without_mapping = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/imports"))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"format": "csv", "content": "title\nA paper\n"}).to_string(),
        ))
        .unwrap();
    assert_eq!(
        call(&pool, csv_without_mapping).await.status(),
        StatusCode::BAD_REQUEST
    );

    let oversized_import = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/imports"))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "format": "doi",
                "content": "x".repeat(2 * 1024 * 1024 + 1)
            })
            .to_string(),
        ))
        .unwrap();
    assert_eq!(
        call(&pool, oversized_import).await.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );

    let list = Request::builder()
        .uri(format!("/projects/{project_id}/acquisitions?limit=1"))
        .body(Body::empty())
        .unwrap();
    let list = call(&pool, list).await;
    assert_eq!(list.status(), StatusCode::OK);
    let list = json_body(list).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert!(list["next_cursor"].is_null());

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn doi_acquisition_persists_provenance_before_durable_jobs_are_claimable() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'acquisition traversal test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
    let request = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/acquisitions"))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"seed_dois": [
                "https://doi.org/10.5555/traversal",
                "DOI:10.5555/TRAVERSAL.",
                "10.5555/traversal"
            ]})
            .to_string(),
        ))
        .unwrap();
    let response = call(&pool, request).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_body(response).await;
    let run_id = body["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    assert_eq!(body["seed_count"], 1);
    assert_eq!(body["queued_count"], 1);
    let linked: Option<Uuid> =
        sqlx::query_scalar("SELECT acquisition_run_id FROM records WHERE acquisition_run_id=$1")
            .bind(run_id)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert!(linked.is_none());
    let run_exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM acquisition_runs WHERE id=$1)")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='work_fetch_requested' AND payload->'payload'->>'ingestion_id'=$1",
    )
    .bind(run_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(run_exists);
    assert_eq!(jobs, 1);
    let items: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ingestion_items WHERE ingestion_id=$1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(items, 1);
    let legacy_link: Uuid =
        sqlx::query_scalar("SELECT legacy_ingestion_id FROM acquisition_runs WHERE id=$1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(legacy_link, run_id);
    sqlx::query("DELETE FROM jobs WHERE payload->'payload'->>'ingestion_id'=$1")
        .bind(run_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn create_acquisition_idempotency_is_replay_safe_under_concurrency() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'concurrent acquisition test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();

    let request = || {
        Request::builder()
            .method("POST")
            .uri(format!("/projects/{project_id}/acquisitions"))
            .header("content-type", "application/json")
            .header("idempotency-key", "concurrent-acquisition")
            .body(Body::from(
                json!({"seed_dois": ["10.5555/concurrent"]}).to_string(),
            ))
            .unwrap()
    };
    let (first, second) = tokio::join!(call(&pool, request()), call(&pool, request()));
    let statuses = [first.status(), second.status()];
    assert!(statuses.contains(&StatusCode::CREATED));
    assert!(statuses.contains(&StatusCode::OK));

    let acquisition_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM acquisition_runs WHERE project_id=$1 AND idempotency_key=$2",
    )
    .bind(project_id)
    .bind("concurrent-acquisition")
    .fetch_one(&pool)
    .await
    .unwrap();
    let ingestion_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ingestions WHERE project_id=$1")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let job_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='work_fetch_requested' AND payload->'payload'->>'project_id'=$1",
    )
    .bind(project_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(acquisition_count, 1);
    assert_eq!(ingestion_count, 1);
    assert_eq!(job_count, 1);

    let conflict_request = |doi: &str| {
        Request::builder()
            .method("POST")
            .uri(format!("/projects/{project_id}/acquisitions"))
            .header("content-type", "application/json")
            .header("idempotency-key", "concurrent-acquisition-conflict")
            .body(Body::from(json!({"seed_dois": [doi]}).to_string()))
            .unwrap()
    };
    let (conflict_first, conflict_second) = tokio::join!(
        call(&pool, conflict_request("10.5555/first")),
        call(&pool, conflict_request("10.5555/second")),
    );
    let conflict_statuses = [conflict_first.status(), conflict_second.status()];
    assert!(conflict_statuses.contains(&StatusCode::CREATED));
    assert!(conflict_statuses.contains(&StatusCode::CONFLICT));

    sqlx::query("DELETE FROM jobs WHERE payload->'payload'->>'project_id'=$1")
        .bind(project_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn importing_into_a_missing_project_returns_not_found() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    let request = Request::builder()
        .method("POST")
        .uri(format!("/projects/{project_id}/imports"))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"format": "doi", "content": "10.5555/missing-project"}).to_string(),
        ))
        .unwrap();
    assert_eq!(call(&pool, request).await.status(), StatusCode::NOT_FOUND);
}
