use std::{path::Path, sync::Arc, time::Duration};

use deepref_application::jobs::ClaimedJob;
use deepref_documents::{
    DocumentParser, DocumentStore, PARSER_VERSION, ParsedBlock, ParsedDocument, ParsedPage,
    PdfParserError, RemoteDocumentFetcher, RemoteFetchError, content_sha256,
};
use deepref_postgres::NewDocument;
use deepref_worker::{
    delivery::DeliveryAction,
    processor::{handle_job_with_document_services, handle_job_with_documents},
};
use futures::stream;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::sync::{Barrier, Mutex};
use uuid::Uuid;

struct FakeParser;

struct FakeFetcher {
    fail: bool,
}

struct RaceFetcher {
    barrier: Arc<Barrier>,
    stored_keys: Arc<Mutex<Vec<String>>>,
}

impl RemoteDocumentFetcher for FakeFetcher {
    fn fetch<'a>(
        &'a self,
        _input: &'a str,
        store: &'a DocumentStore,
    ) -> deepref_documents::RemoteFetchFuture<'a> {
        Box::pin(async move {
            if self.fail {
                return Err(RemoteFetchError::InvalidSignature);
            }
            let stored = store
                .put_stream(stream::iter([Ok::<_, String>(bytes::Bytes::from_static(
                    b"%PDF-1.7\nremote fixture",
                ))]))
                .await?;
            Ok((
                stored,
                url::Url::parse("https://example.test/study.pdf").unwrap(),
            ))
        })
    }
}

impl RemoteDocumentFetcher for RaceFetcher {
    fn fetch<'a>(
        &'a self,
        _input: &'a str,
        store: &'a DocumentStore,
    ) -> deepref_documents::RemoteFetchFuture<'a> {
        Box::pin(async move {
            let stored = store
                .put_stream(stream::iter([Ok::<_, String>(bytes::Bytes::from_static(
                    b"%PDF-1.7\nrace fixture",
                ))]))
                .await?;
            self.stored_keys.lock().await.push(stored.opaque_id.clone());
            self.barrier.wait().await;
            Ok((
                stored,
                url::Url::parse("https://example.test/race.pdf").unwrap(),
            ))
        })
    }
}

impl DocumentParser for FakeParser {
    fn version(&self) -> &'static str {
        PARSER_VERSION
    }

    fn parse_file(&self, path: &Path) -> Result<ParsedDocument, PdfParserError> {
        let bytes =
            std::fs::read(path).map_err(|error| PdfParserError::Document(error.to_string()))?;
        if !bytes.starts_with(b"%PDF-") {
            return Err(PdfParserError::Document("invalid fixture".to_owned()));
        }
        Ok(ParsedDocument {
            pages: vec![
                ParsedPage {
                    page_number: 1,
                    width: 612.0,
                    height: 792.0,
                    ocr_required: true,
                },
                ParsedPage {
                    page_number: 2,
                    width: 612.0,
                    height: 792.0,
                    ocr_required: false,
                },
            ],
            ocr_required: true,
            blocks: vec![ParsedBlock {
                page_number: 2,
                page_width: 612.0,
                page_height: 792.0,
                ordinal: 0,
                kind: "text".to_owned(),
                text: "Eligibility evidence".to_owned(),
                bbox: deepref_domain::NormalizedBoundingBox::new(0.1, 0.2, 0.3, 0.1).ok(),
                content_hash: content_sha256(b"Eligibility evidence"),
            }],
        })
    }
}

#[tokio::test]
async fn retrieve_document_is_durable_idempotent_and_records_terminal_failure() {
    let Some(pool) = database().await else { return };
    let project_id = Uuid::new_v4();
    let report_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'retrieve worker test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO reports(id,title) VALUES($1,'retrieve report')")
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
    let store = Arc::new(DocumentStore::memory());

    for (fail, expected_status) in [(false, "uploaded"), (true, "failed")] {
        let document_id = Uuid::new_v4();
        let external_url = format!("https://example.test/{document_id}.pdf");
        let mut transaction = pool.begin().await.unwrap();
        deepref_postgres::create_document(
            &mut transaction,
            NewDocument {
                project_id,
                report_id,
                id: document_id,
                source: "external_url",
                status: "external",
                original_filename: None,
                external_url: Some(&external_url),
                mime_type: "application/pdf",
                byte_size: 0,
                content_hash: None,
                object_key: None,
                actor_kind: "system",
                actor_id: "retrieve-test",
            },
        )
        .await
        .unwrap();
        transaction.commit().await.unwrap();
        let job = ClaimedJob {
            id: Uuid::new_v4(),
            kind: "retrieve_document".to_owned(),
            payload: serde_json::json!({"document_id": document_id}),
            attempts: 1,
            max_attempts: 5,
        };
        let result = handle_job_with_document_services(
            pool.clone(),
            &job,
            Duration::from_secs(30),
            Some(Arc::clone(&store)),
            None,
            Some(Arc::new(FakeFetcher { fail })),
        )
        .await;
        assert_eq!(result.is_err(), fail);
        let document = deepref_postgres::get_document_by_id(&pool, document_id)
            .await
            .unwrap();
        assert_eq!(document.status, expected_status);
        if fail {
            assert!(document.object_key.is_none());
            assert!(document.content_hash.is_none());
        } else {
            assert!(document.object_key.is_some());
            assert!(document.content_hash.is_some());
            let parse_jobs: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM jobs WHERE kind='parse_document' AND payload->>'document_id'=$1",
            )
            .bind(document_id.to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(parse_jobs, 1);
            let second = handle_job_with_document_services(
                pool.clone(),
                &job,
                Duration::from_secs(30),
                Some(Arc::clone(&store)),
                None,
                Some(Arc::new(FakeFetcher { fail: false })),
            )
            .await
            .unwrap();
            assert_eq!(second, DeliveryAction::Ack);
        }
    }
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn concurrent_retrieval_completion_cleans_the_losing_object() {
    let Some(pool) = database().await else { return };
    let project_id = Uuid::new_v4();
    let report_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'retrieve race test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO reports(id,title) VALUES($1,'retrieve race report')")
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
    let external_url = format!("https://example.test/{document_id}.pdf");
    let mut transaction = pool.begin().await.unwrap();
    deepref_postgres::create_document(
        &mut transaction,
        NewDocument {
            project_id,
            report_id,
            id: document_id,
            source: "external_url",
            status: "external",
            original_filename: None,
            external_url: Some(&external_url),
            mime_type: "application/pdf",
            byte_size: 0,
            content_hash: None,
            object_key: None,
            actor_kind: "system",
            actor_id: "retrieval-race-test",
        },
    )
    .await
    .unwrap();
    transaction.commit().await.unwrap();

    let store = Arc::new(DocumentStore::memory());
    let stored_keys = Arc::new(Mutex::new(Vec::new()));
    let fetcher: Arc<dyn RemoteDocumentFetcher> = Arc::new(RaceFetcher {
        barrier: Arc::new(Barrier::new(2)),
        stored_keys: Arc::clone(&stored_keys),
    });
    let job = ClaimedJob {
        id: Uuid::new_v4(),
        kind: "retrieve_document".to_owned(),
        payload: serde_json::json!({"document_id": document_id}),
        attempts: 1,
        max_attempts: 5,
    };
    let first = handle_job_with_document_services(
        pool.clone(),
        &job,
        Duration::from_secs(30),
        Some(Arc::clone(&store)),
        None,
        Some(Arc::clone(&fetcher)),
    );
    let second = handle_job_with_document_services(
        pool.clone(),
        &job,
        Duration::from_secs(30),
        Some(Arc::clone(&store)),
        None,
        Some(Arc::clone(&fetcher)),
    );
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first.unwrap(), DeliveryAction::Ack);
    assert_eq!(second.unwrap(), DeliveryAction::Ack);

    let document = deepref_postgres::get_document_by_id(&pool, document_id)
        .await
        .unwrap();
    assert_eq!(document.status, "uploaded");
    let authoritative_key = document.object_key.clone().unwrap();
    let keys = stored_keys.lock().await.clone();
    assert_eq!(keys.len(), 2);
    assert!(keys.contains(&authoritative_key));
    for key in &keys {
        let exists = store.get(key).await.is_ok();
        assert_eq!(exists, key == &authoritative_key);
    }
    let parse_jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='parse_document' AND payload->>'document_id'=$1",
    )
    .bind(document_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(parse_jobs, 1);

    let retry = handle_job_with_document_services(
        pool.clone(),
        &job,
        Duration::from_secs(30),
        Some(Arc::clone(&store)),
        None,
        Some(fetcher),
    )
    .await
    .unwrap();
    assert_eq!(retry, DeliveryAction::Ack);
    assert_eq!(stored_keys.lock().await.len(), 2);
    assert_eq!(
        deepref_postgres::get_document_by_id(&pool, document_id)
            .await
            .unwrap()
            .object_key
            .as_deref(),
        Some(authoritative_key.as_str())
    );

    store.delete(&authoritative_key).await.unwrap();
    sqlx::query("DELETE FROM jobs WHERE payload->>'document_id'=$1")
        .bind(document_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

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

#[tokio::test]
async fn parse_document_is_streamed_versioned_and_idempotent() {
    let Some(pool) = database().await else { return };
    let project_id = Uuid::new_v4();
    let report_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'worker document test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO reports(id,title) VALUES($1,'worker report')")
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

    let store = DocumentStore::memory();
    let bytes = bytes::Bytes::from_static(b"%PDF-1.7\nfixture");
    let stored = store
        .put_stream(stream::iter([Ok::<_, String>(bytes)]))
        .await
        .unwrap();
    let document_id = Uuid::new_v4();
    let mut tx = pool.begin().await.unwrap();
    deepref_postgres::create_document(
        &mut tx,
        NewDocument {
            project_id,
            report_id,
            id: document_id,
            source: "upload",
            status: "uploaded",
            original_filename: Some("fixture.pdf"),
            external_url: None,
            mime_type: "application/pdf",
            byte_size: stored.byte_size as i64,
            content_hash: Some(&stored.sha256),
            object_key: Some(&stored.opaque_id),
            actor_kind: "system",
            actor_id: "worker-test",
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let job = ClaimedJob {
        id: Uuid::new_v4(),
        kind: "parse_document".to_owned(),
        payload: serde_json::json!({"document_id": document_id, "parser_version": PARSER_VERSION}),
        attempts: 1,
        max_attempts: 5,
    };
    let store = Arc::new(store);
    let parser: Arc<dyn DocumentParser> = Arc::new(FakeParser);
    let first = handle_job_with_documents(
        pool.clone(),
        &job,
        Duration::from_secs(30),
        Some(Arc::clone(&store)),
        Some(Arc::clone(&parser)),
    )
    .await
    .unwrap();
    assert_eq!(first, DeliveryAction::Ack);
    let second = handle_job_with_documents(
        pool.clone(),
        &job,
        Duration::from_secs(30),
        Some(store),
        Some(parser),
    )
    .await
    .unwrap();
    assert_eq!(second, DeliveryAction::Ack);
    let block_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_blocks WHERE document_id=$1 AND active")
            .bind(document_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(block_count, 1);
    let page_flags: Vec<(i32, bool)> = sqlx::query_as(
        "SELECT page_number,ocr_required FROM document_pages WHERE document_id=$1 AND active ORDER BY page_number",
    )
    .bind(document_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(page_flags, vec![(1, true), (2, false)]);
    let document = deepref_postgres::get_document_by_id(&pool, document_id)
        .await
        .unwrap();
    assert_eq!(document.status, "available");
    assert!(document.ocr_required);
    assert_eq!(
        document.active_parser_version.as_deref(),
        Some(PARSER_VERSION)
    );
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}
