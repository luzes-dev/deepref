#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use std::collections::HashMap;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use deepref_documents::{DocumentStore, StoreConfig};
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .ok()?;
    deepref_postgres::migrate(&pool).await.ok()?;
    Some(pool)
}

fn api_config() -> ApiConfig {
    let runtime = deepref_config::RuntimeConfig::from_map(
        "deepref-api-document-test",
        &HashMap::from([("APP_ENV".to_owned(), "local".to_owned())]),
    )
    .unwrap();
    ApiConfig {
        runtime,
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        cors_allow_any: false,
        cors_origins: Vec::new(),
    }
}

fn multipart(pdf: &[u8], filename: &str) -> (String, Body) {
    let boundary = "deepref-document-boundary";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/pdf\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(pdf);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (
        format!("multipart/form-data; boundary={boundary}"),
        Body::from(body),
    )
}

#[tokio::test]
async fn document_http_upload_is_bounded_scoped_deduped_and_opaque() {
    let Some(pool) = database().await else { return };
    let project_id = Uuid::new_v4();
    let other_project_id = Uuid::new_v4();
    let report_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'documents api'),($2,'other')")
        .bind(project_id)
        .bind(other_project_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO reports(id,title) VALUES($1,'document api report')")
        .bind(report_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
        .bind(project_id)
        .bind(report_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO screening_state(project_id,report_id,title_abstract_status,full_text_status,final_status)
         VALUES($1,$2,'include','unscreened','pending_full_text')",
    )
    .bind(project_id)
    .bind(report_id)
    .execute(&pool)
    .await
    .unwrap();
    let store = DocumentStore::new(StoreConfig::memory(), 128);
    let state = AppState::new(pool.clone()).with_document_store(store);
    let pdf = b"%PDF-1.7\nsmall deterministic fixture";
    let (content_type, body) = multipart(pdf, "../../study.pdf");
    let upload = router(state.clone(), &api_config())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/projects/{project_id}/reports/{report_id}/documents"
                ))
                .header("content-type", content_type)
                .header("x-actor-id", "document-http-test")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(upload.status(), StatusCode::CREATED);
    let value: serde_json::Value =
        serde_json::from_slice(&to_bytes(upload.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(value["original_filename"], "study.pdf");
    assert!(value.get("object_key").is_none());
    let document_id: Uuid = value["id"].as_str().unwrap().parse().unwrap();
    let row = sqlx::query("SELECT object_key,content_hash FROM documents WHERE id=$1")
        .bind(document_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let object_key: String = row.get("object_key");
    assert!(object_key.starts_with("documents/"));
    assert!(!object_key.contains("study"));
    assert_eq!(row.get::<String, _>("content_hash").len(), 64);

    let (duplicate_type, duplicate_body) = multipart(pdf, "duplicate.pdf");
    let duplicate = router(state.clone(), &api_config())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/projects/{project_id}/reports/{report_id}/documents"
                ))
                .header("content-type", duplicate_type)
                .body(duplicate_body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    let attached_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM documents WHERE project_id=$1 AND report_id=$2 AND content_hash=$3",
    )
    .bind(project_id)
    .bind(report_id)
    .bind(row.get::<String, _>("content_hash"))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attached_count, 1);

    let content = router(state.clone(), &api_config())
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/projects/{project_id}/reports/{report_id}/documents/{document_id}/content"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(content.status(), StatusCode::OK);
    assert_eq!(content.headers()["content-length"], pdf.len().to_string());
    assert_eq!(content.headers()["cache-control"], "private, no-store");
    assert_eq!(
        to_bytes(content.into_body(), 128).await.unwrap().as_ref(),
        pdf
    );
    let isolated = router(state.clone(), &api_config())
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/projects/{other_project_id}/reports/{report_id}/documents/{document_id}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(isolated.status(), StatusCode::NOT_FOUND);

    let ssrf = router(state.clone(), &api_config())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/projects/{project_id}/reports/{report_id}/documents/external"
                ))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"https://127.0.0.1/private.pdf"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ssrf.status(), StatusCode::BAD_REQUEST);

    let attached = router(state.clone(), &api_config())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/projects/{project_id}/reports/{report_id}/documents/external"
                ))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"https://example.com/study.pdf"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(attached.status(), StatusCode::CREATED);
    let attached: serde_json::Value =
        serde_json::from_slice(&to_bytes(attached.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(attached["status"], "external");
    let retrieve_jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='retrieve_document' AND payload->>'document_id'=$1",
    )
    .bind(attached["id"].as_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(retrieve_jobs, 1);

    let queue = router(state.clone(), &api_config())
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/projects/{project_id}/screening/full-text?limit=10"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(queue.status(), StatusCode::OK);
    let queue: serde_json::Value =
        serde_json::from_slice(&to_bytes(queue.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(queue["items"][0]["report_id"], report_id.to_string());
    assert_eq!(queue["items"][0]["document"]["status"], "external");

    let reasons = router(state, &api_config())
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/projects/{project_id}/screening/full-text/reasons"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reasons.status(), StatusCode::OK);
    let reasons: Vec<serde_json::Value> =
        serde_json::from_slice(&to_bytes(reasons.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(reasons.len(), 8);
    sqlx::query("DELETE FROM projects WHERE id IN($1,$2)")
        .bind(project_id)
        .bind(other_project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn oversized_upload_leaves_no_database_document() {
    let Some(pool) = database().await else { return };
    let project_id = Uuid::new_v4();
    let report_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'bounded upload')")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO reports(id,title) VALUES($1,'bounded report')")
        .bind(report_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
        .bind(project_id)
        .bind(report_id)
        .execute(&pool)
        .await
        .unwrap();
    let state = AppState::new(pool.clone())
        .with_document_store(DocumentStore::new(StoreConfig::memory(), 8));
    let (content_type, body) = multipart(b"%PDF-this-is-too-large", "large.pdf");
    let response = router(state, &api_config())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/projects/{project_id}/reports/{report_id}/documents"
                ))
                .header("content-type", content_type)
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE project_id=$1")
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn document_upload_has_its_own_limit_above_the_acquisition_default() {
    let Some(pool) = database().await else { return };
    let project_id = Uuid::new_v4();
    let report_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'large upload')")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO reports(id,title) VALUES($1,'large bounded report')")
        .bind(report_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
        .bind(project_id)
        .bind(report_id)
        .execute(&pool)
        .await
        .unwrap();
    let maximum = 14 * 1024 * 1024;
    let state = AppState::new(pool.clone())
        .with_document_store(DocumentStore::new(StoreConfig::memory(), maximum));
    let mut pdf = vec![0_u8; 13 * 1024 * 1024];
    pdf[..5].copy_from_slice(b"%PDF-");
    let (content_type, body) = multipart(&pdf, "large.pdf");
    let response = router(state, &api_config())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/projects/{project_id}/reports/{report_id}/documents"
                ))
                .header("content-type", content_type)
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}
