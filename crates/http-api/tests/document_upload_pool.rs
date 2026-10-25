#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Uploads must not acquire a second pool connection while their durable
//! upload transaction is held, even when every pool slot owns an upload.
use std::{collections::HashMap, convert::Infallible, sync::Arc, time::Duration};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use bytes::Bytes;
use deepref_documents::DocumentStore;
use deepref_http_api::{config::ApiConfig, routes::router, state::AppState};
use futures::{Stream, stream};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::sync::{Barrier, mpsc};
use tower::ServiceExt;
use uuid::Uuid;

async fn database(maximum: u32) -> Option<PgPool> {
    let url = deepref_app::test_isolation::isolated_url_from_env("DATABASE_URL").await?;
    Some(
        PgPoolOptions::new()
            .max_connections(maximum)
            .acquire_timeout(Duration::from_secs(1))
            .connect(&url)
            .await
            .unwrap(),
    )
}

fn api_config() -> ApiConfig {
    let runtime = deepref_config::RuntimeConfig::from_map(
        "document-upload-pool-test",
        &HashMap::from([("APP_ENV".to_owned(), "development".to_owned())]),
    )
    .unwrap();
    ApiConfig {
        documents: deepref_config::DocumentConfig::from_source(&|name| {
            (name == "APP_ENV").then(|| "development".into())
        })
        .unwrap(),
        runtime,
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        cors_allow_any: false,
        cors_origins: Vec::new(),
    }
}

async fn fixture(pool: &PgPool, reports: usize) -> (Uuid, Vec<Uuid>) {
    let project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'upload pool regression')")
        .bind(project)
        .execute(pool)
        .await
        .unwrap();
    let mut ids = Vec::new();
    for _ in 0..reports {
        let report = Uuid::new_v4();
        sqlx::query("INSERT INTO reports(id,title) VALUES($1,'pool fixture')")
            .bind(report)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
            .bind(project)
            .bind(report)
            .execute(pool)
            .await
            .unwrap();
        ids.push(report);
    }
    (project, ids)
}

fn payload(pdf: &str) -> Bytes {
    Bytes::from(format!(
        "--pool-boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"fixture.pdf\"\r\nContent-Type: application/pdf\r\n\r\n{pdf}\r\n--pool-boundary--\r\n"
    ))
}

fn request(project: Uuid, report: Uuid, body: Body) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/projects/{project}/reports/{report}/documents"))
        .header(
            "content-type",
            "multipart/form-data; boundary=pool-boundary",
        )
        .body(body)
        .unwrap()
}

async fn upload(state: AppState, project: Uuid, report: Uuid, body: Body) -> Uuid {
    let response = tokio::time::timeout(
        Duration::from_secs(10),
        router(state, &api_config()).oneshot(request(project, report, body)),
    )
    .await
    .expect("upload must complete without a pool hold-and-wait")
    .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    assert_eq!(
        status,
        StatusCode::CREATED,
        "upload response: {}",
        String::from_utf8_lossy(&bytes)
    );
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["id"].as_str().unwrap().parse().unwrap()
}

#[tokio::test]
async fn ordinary_and_failed_hash_retry_uploads_succeed_with_one_pool_connection() {
    let Some(pool) = database(1).await else {
        return;
    };
    let (project, reports) = fixture(&pool, 1).await;
    let state = AppState::new(pool.clone()).with_document_store(DocumentStore::memory());
    let body = payload("%PDF-1.4\none connection");
    let id = upload(state.clone(), project, reports[0], Body::from(body.clone())).await;
    deepref_app::mark_document_failed(&pool, id, "injected parse failure")
        .await
        .unwrap();
    let retried = upload(state, project, reports[0], Body::from(body)).await;
    assert_eq!(
        retried, id,
        "identical failed content must reuse its document"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE project_id=$1")
        .bind(project)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

fn held_upload_body(
    bytes: Bytes,
    entered: mpsc::Sender<()>,
    release: Arc<Barrier>,
) -> impl Stream<Item = Result<Bytes, Infallible>> + Send {
    stream::once(async move {
        // Multipart consumption starts inside upload_document, after its
        // upload transaction owns a pool slot and the registry row lock.
        entered.send(()).await.unwrap();
        release.wait().await;
        Ok(bytes)
    })
}

#[tokio::test]
async fn a_pool_sized_upload_wave_completes_without_a_second_connection() {
    const POOL_SIZE: usize = 3;
    let Some(pool) = database(POOL_SIZE as u32).await else {
        return;
    };
    let (project, reports) = fixture(&pool, POOL_SIZE).await;
    let state = AppState::new(pool.clone()).with_document_store(DocumentStore::memory());
    let release = Arc::new(Barrier::new(POOL_SIZE + 1));
    let (entered_tx, mut entered_rx) = mpsc::channel(POOL_SIZE);
    let mut uploads = Vec::new();
    for (index, report) in reports.into_iter().enumerate() {
        let body = Body::from_stream(held_upload_body(
            payload(&format!("%PDF-1.4\nwave {index}")),
            entered_tx.clone(),
            Arc::clone(&release),
        ));
        uploads.push(tokio::spawn(upload(state.clone(), project, report, body)));
    }
    drop(entered_tx);
    for _ in 0..POOL_SIZE {
        tokio::time::timeout(Duration::from_secs(10), entered_rx.recv())
            .await
            .expect("every upload must reach the held transaction barrier")
            .expect("upload body entered");
    }
    assert_eq!(pool.size(), POOL_SIZE as u32);
    assert_eq!(
        pool.num_idle(),
        0,
        "the entire wave must own its upload transactions before release"
    );
    release.wait().await;
    for task in uploads {
        task.await.expect("pool-sized upload must succeed");
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE project_id=$1")
        .bind(project)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, POOL_SIZE as i64);
}
