#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use deepref_documents::{ParsedBlock, ParsedDocument, ParsedPage};
use deepref_postgres::{
    CompleteDocumentRetrievalOutcome, NewDocument, complete_document_retrieval, create_document,
    enqueue_parse, get_document, get_document_by_id, insert_document_blocks, list_documents,
    list_full_text_queue, list_full_text_reasons, list_missing_full_text, mark_document_retrieving,
    migrate, persist_parsed_document, search_document_blocks,
};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use uuid::Uuid;

const DOCUMENT_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const BLOCK_HASH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .ok()?;
    migrate(&pool).await.ok()?;
    Some(pool)
}

async fn fixture(pool: &PgPool) -> (Uuid, Uuid) {
    let project_id = Uuid::new_v4();
    let report_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'document fixture')")
        .bind(project_id)
        .execute(pool)
        .await
        .expect("project should insert");
    sqlx::query(
        "INSERT INTO reports (id,title,abstract_text) VALUES ($1,'Document report','Abstract')",
    )
    .bind(report_id)
    .execute(pool)
    .await
    .expect("report should insert");
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project_id)
        .bind(report_id)
        .execute(pool)
        .await
        .expect("membership should insert");
    sqlx::query("INSERT INTO screening_state (project_id,report_id,title_abstract_status,full_text_status,final_status) VALUES ($1,$2,'include','unscreened','pending_full_text')")
        .bind(project_id)
        .bind(report_id)
        .execute(pool)
        .await
        .expect("screening state should insert");
    (project_id, report_id)
}

async fn cleanup(pool: &PgPool, project_id: Uuid) {
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(pool)
        .await
        .expect("fixture should clean up");
}

#[tokio::test]
async fn uploaded_document_has_project_scope_blocks_and_audited_metadata() {
    let Some(pool) = database().await else { return };
    let (project_id, report_id) = fixture(&pool).await;
    let document_id = Uuid::new_v4();
    let object_key = format!("documents/{document_id}");
    let mut tx = pool.begin().await.expect("transaction should begin");
    create_document(
        &mut tx,
        NewDocument {
            project_id,
            report_id,
            id: document_id,
            source: "upload",
            status: "uploaded",
            original_filename: Some("study.pdf"),
            external_url: None,
            mime_type: "application/pdf",
            byte_size: 123,
            content_hash: Some(DOCUMENT_HASH),
            object_key: Some(&object_key),
            actor_kind: "user",
            actor_id: "document-tester",
        },
    )
    .await
    .expect("document should insert");
    tx.commit().await.expect("transaction should commit");
    let parsed = ParsedDocument {
        pages: vec![ParsedPage {
            page_number: 1,
            width: 612.0,
            height: 792.0,
            ocr_required: false,
        }],
        ocr_required: false,
        blocks: vec![ParsedBlock {
            page_number: 1,
            page_width: 612.0,
            page_height: 792.0,
            ordinal: 0,
            kind: "text".to_owned(),
            text: "Eligible population".to_owned(),
            bbox: None,
            content_hash: BLOCK_HASH.to_owned(),
        }],
    };
    insert_document_blocks(&pool, document_id, &parsed)
        .await
        .expect("blocks should persist");
    let documents = list_documents(&pool, project_id, Some(report_id), 10)
        .await
        .expect("documents should list");
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].status, "available");
    assert_eq!(documents[0].content_hash.as_deref(), Some(DOCUMENT_HASH));
    let missing = list_missing_full_text(&pool, project_id, 10)
        .await
        .expect("missing queue should load");
    assert!(missing.is_empty());
    let block_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_blocks WHERE document_id=$1")
            .bind(document_id)
            .fetch_one(&pool)
            .await
            .expect("blocks should count");
    assert_eq!(block_count, 1);
    let search =
        search_document_blocks(&pool, project_id, report_id, document_id, "population", 10)
            .await
            .expect("full-text search should load");
    assert_eq!(search.len(), 1);
    cleanup(&pool, project_id).await;
}

#[tokio::test]
async fn missing_full_text_queue_is_project_scoped_and_excludes_available_documents() {
    let Some(pool) = database().await else { return };
    let (project_id, report_id) = fixture(&pool).await;
    let other_project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'other document fixture')")
        .bind(other_project)
        .execute(&pool)
        .await
        .expect("other project should insert");
    let missing = list_missing_full_text(&pool, project_id, 10)
        .await
        .expect("missing queue should load");
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].report_id, report_id);
    let document_id = Uuid::new_v4();
    let object_key = format!("documents/{document_id}");
    let mut tx = pool.begin().await.expect("transaction should begin");
    create_document(
        &mut tx,
        NewDocument {
            project_id,
            report_id,
            id: document_id,
            source: "external_url",
            status: "uploaded",
            original_filename: None,
            external_url: Some("https://example.com/study.pdf"),
            mime_type: "application/pdf",
            byte_size: 321,
            content_hash: Some("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"),
            object_key: Some(&object_key),
            actor_kind: "user",
            actor_id: "document-tester",
        },
    )
    .await
    .expect("external document should insert");
    tx.commit().await.expect("transaction should commit");
    let missing_after_external = list_missing_full_text(&pool, project_id, 10)
        .await
        .expect("missing queue should load");
    assert_eq!(missing_after_external.len(), 0);
    assert!(
        get_document(&pool, other_project, report_id, document_id)
            .await
            .is_err()
    );
    let project_id_from_db: Uuid = sqlx::query("SELECT project_id FROM documents WHERE id=$1")
        .bind(document_id)
        .fetch_one(&pool)
        .await
        .expect("document should exist")
        .get("project_id");
    assert_eq!(project_id_from_db, project_id);
    cleanup(&pool, project_id).await;
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(other_project)
        .execute(&pool)
        .await
        .expect("other fixture should clean up");
}

#[tokio::test]
async fn migration_seeds_complete_reasons_and_enforces_reason_shape() {
    let Some(pool) = database().await else { return };
    let (project_id, report_id) = fixture(&pool).await;
    let reasons = list_full_text_reasons(&pool, project_id)
        .await
        .expect("reason catalog should load");
    assert_eq!(reasons.len(), 8);
    assert!(
        reasons
            .iter()
            .any(|reason| reason.code == "wrong_comparator_outcome")
    );
    assert!(reasons.iter().any(|reason| reason.code == "other"));
    let reason_id = reasons[0].id;

    let include_with_reason = sqlx::query(
        "UPDATE screening_state SET full_text_status='include',
           full_text_exclusion_reason_id=$3 WHERE project_id=$1 AND report_id=$2",
    )
    .bind(project_id)
    .bind(report_id)
    .bind(reason_id)
    .execute(&pool)
    .await;
    assert!(include_with_reason.is_err());

    let exclude_without_reason = sqlx::query(
        "UPDATE screening_state SET full_text_status='exclude',
           full_text_exclusion_reason_id=NULL WHERE project_id=$1 AND report_id=$2",
    )
    .bind(project_id)
    .bind(report_id)
    .execute(&pool)
    .await;
    assert!(exclude_without_reason.is_err());

    sqlx::query(
        "UPDATE screening_state SET full_text_status='exclude',
           full_text_exclusion_reason_id=$3 WHERE project_id=$1 AND report_id=$2",
    )
    .bind(project_id)
    .bind(report_id)
    .bind(reason_id)
    .execute(&pool)
    .await
    .expect("exclude with one project reason should persist");
    let other_project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'reason isolation')")
        .bind(other_project)
        .execute(&pool)
        .await
        .unwrap();
    let other_reason: Uuid = sqlx::query_scalar(
        "SELECT id FROM exclusion_reasons WHERE project_id=$1 AND stage='full_text' LIMIT 1",
    )
    .bind(other_project)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        sqlx::query(
            "UPDATE screening_state SET full_text_exclusion_reason_id=$3
             WHERE project_id=$1 AND report_id=$2",
        )
        .bind(project_id)
        .bind(report_id)
        .bind(other_reason)
        .execute(&pool)
        .await
        .is_err()
    );
    let title_reason = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO exclusion_reasons(id,project_id,code,label,stage)
         VALUES($1,$2,'title_only','Title only','title_abstract')",
    )
    .bind(title_reason)
    .bind(project_id)
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        sqlx::query(
            "UPDATE screening_state SET full_text_exclusion_reason_id=$3
             WHERE project_id=$1 AND report_id=$2",
        )
        .bind(project_id)
        .bind(report_id)
        .bind(title_reason)
        .execute(&pool)
        .await
        .is_err()
    );
    cleanup(&pool, project_id).await;
    cleanup(&pool, other_project).await;
}

#[tokio::test]
async fn parse_jobs_dedupe_and_failed_new_version_preserves_active_blocks() {
    let Some(pool) = database().await else { return };
    let (project_id, report_id) = fixture(&pool).await;
    let document_id = Uuid::new_v4();
    let object_key = format!("documents/{document_id}");
    let mut tx = pool.begin().await.unwrap();
    create_document(
        &mut tx,
        NewDocument {
            project_id,
            report_id,
            id: document_id,
            source: "upload",
            status: "uploaded",
            original_filename: None,
            external_url: None,
            mime_type: "application/pdf",
            byte_size: 12,
            content_hash: Some("dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"),
            object_key: Some(&object_key),
            actor_kind: "system",
            actor_id: "parser-test",
        },
    )
    .await
    .unwrap();
    let first_job = enqueue_parse(&mut tx, project_id.into(), document_id, DOCUMENT_HASH)
        .await
        .unwrap();
    let second_job = enqueue_parse(&mut tx, project_id.into(), document_id, DOCUMENT_HASH)
        .await
        .unwrap();
    assert_eq!(first_job, second_job);
    tx.commit().await.unwrap();

    let version_one = ParsedDocument {
        pages: vec![ParsedPage {
            page_number: 1,
            width: 600.0,
            height: 800.0,
            ocr_required: false,
        }],
        ocr_required: false,
        blocks: vec![ParsedBlock {
            page_number: 1,
            page_width: 600.0,
            page_height: 800.0,
            ordinal: 0,
            kind: "text".to_owned(),
            text: "authoritative text".to_owned(),
            bbox: deepref_domain::NormalizedBoundingBox::new(0.1, 0.1, 0.5, 0.1).ok(),
            content_hash: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
                .to_owned(),
        }],
    };
    persist_parsed_document(&pool, document_id, &version_one, "parser-v1")
        .await
        .unwrap();
    let invalid_version = ParsedDocument {
        pages: version_one.pages.clone(),
        ocr_required: false,
        blocks: vec![ParsedBlock {
            content_hash: "invalid".to_owned(),
            text: "replacement".to_owned(),
            ..version_one.blocks[0].clone()
        }],
    };
    assert!(
        persist_parsed_document(&pool, document_id, &invalid_version, "parser-v2")
            .await
            .is_err()
    );
    let active: Vec<(String, String)> = sqlx::query_as(
        "SELECT parser_version,text FROM document_blocks WHERE document_id=$1 AND active",
    )
    .bind(document_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        active,
        vec![("parser-v1".to_owned(), "authoritative text".to_owned())]
    );
    let active_pages: Vec<(String, i32)> = sqlx::query_as(
        "SELECT parser_version,page_number FROM document_pages WHERE document_id=$1 AND active",
    )
    .bind(document_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(active_pages, vec![("parser-v1".to_owned(), 1)]);
    cleanup(&pool, project_id).await;
}

#[tokio::test]
async fn content_hash_dedupe_is_scoped_to_a_report_attachment() {
    let Some(pool) = database().await else { return };
    let (project_id, report_id) = fixture(&pool).await;
    let second_report_id = Uuid::new_v4();
    sqlx::query("INSERT INTO reports(id,title) VALUES($1,'second report')")
        .bind(second_report_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
        .bind(project_id)
        .bind(second_report_id)
        .execute(&pool)
        .await
        .unwrap();

    for (index, target_report) in [report_id, second_report_id].into_iter().enumerate() {
        let mut transaction = pool.begin().await.unwrap();
        create_document(
            &mut transaction,
            NewDocument {
                project_id,
                report_id: target_report,
                id: Uuid::new_v4(),
                source: "upload",
                status: "uploaded",
                original_filename: None,
                external_url: None,
                mime_type: "application/pdf",
                byte_size: 10,
                content_hash: Some(DOCUMENT_HASH),
                object_key: Some(if index == 0 {
                    "documents/44444444-4444-4444-8444-444444444444"
                } else {
                    "documents/55555555-5555-4555-8555-555555555555"
                }),
                actor_kind: "system",
                actor_id: "dedupe-test",
            },
        )
        .await
        .expect("identical content may attach to a different report");
        transaction.commit().await.unwrap();
    }

    let mut duplicate = pool.begin().await.unwrap();
    let result = create_document(
        &mut duplicate,
        NewDocument {
            project_id,
            report_id,
            id: Uuid::new_v4(),
            source: "upload",
            status: "uploaded",
            original_filename: None,
            external_url: None,
            mime_type: "application/pdf",
            byte_size: 10,
            content_hash: Some(DOCUMENT_HASH),
            object_key: Some("documents/66666666-6666-4666-8666-666666666666"),
            actor_kind: "system",
            actor_id: "dedupe-test",
        },
    )
    .await;
    assert!(
        result.is_err(),
        "same report and hash must not fork identity"
    );
    duplicate.rollback().await.unwrap();
    cleanup(&pool, project_id).await;
}

#[tokio::test]
async fn full_text_queue_keeps_reports_selected_through_acquisition_states() {
    let Some(pool) = database().await else { return };
    let (project_id, report_id) = fixture(&pool).await;
    let initial = list_full_text_queue(&pool, project_id, None, None, None, 10)
        .await
        .unwrap();
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].report_id, report_id);
    assert!(initial[0].document.is_none());

    let document_id = Uuid::new_v4();
    let mut transaction = pool.begin().await.unwrap();
    create_document(
        &mut transaction,
        NewDocument {
            project_id,
            report_id,
            id: document_id,
            source: "external_url",
            status: "external",
            original_filename: None,
            external_url: Some("https://example.com/study.pdf"),
            mime_type: "application/pdf",
            byte_size: 0,
            content_hash: None,
            object_key: None,
            actor_kind: "user",
            actor_id: "queue-test",
        },
    )
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    let attached = list_full_text_queue(
        &pool,
        project_id,
        Some("unscreened"),
        Some("document"),
        None,
        10,
    )
    .await
    .unwrap();
    assert_eq!(attached.len(), 1);
    assert_eq!(attached[0].document.as_ref().unwrap().status, "external");
    cleanup(&pool, project_id).await;
}

#[tokio::test]
async fn retrieval_completion_has_one_winner_and_one_parse_job() {
    let Some(pool) = database().await else { return };
    let (project_id, report_id) = fixture(&pool).await;
    let document_id = Uuid::new_v4();
    let external_url = format!("https://example.test/{document_id}.pdf");
    let mut transaction = pool.begin().await.unwrap();
    create_document(
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
    assert!(mark_document_retrieving(&pool, document_id).await.unwrap());

    let winner_key = format!("documents/{}", Uuid::new_v4());
    let mut winner = pool.begin().await.unwrap();
    assert_eq!(
        complete_document_retrieval(
            &mut winner,
            project_id.into(),
            document_id,
            &winner_key,
            DOCUMENT_HASH,
            100,
        )
        .await
        .unwrap(),
        CompleteDocumentRetrievalOutcome::Applied
    );
    winner.commit().await.unwrap();

    let loser_key = format!("documents/{}", Uuid::new_v4());
    let loser_hash = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    let mut loser = pool.begin().await.unwrap();
    assert_eq!(
        complete_document_retrieval(
            &mut loser,
            project_id.into(),
            document_id,
            &loser_key,
            loser_hash,
            101,
        )
        .await
        .unwrap(),
        CompleteDocumentRetrievalOutcome::AlreadyCompleted
    );
    loser.commit().await.unwrap();

    let document = get_document_by_id(&pool, document_id).await.unwrap();
    assert_eq!(document.status, "uploaded");
    assert_eq!(document.object_key.as_deref(), Some(winner_key.as_str()));
    assert_eq!(document.content_hash.as_deref(), Some(DOCUMENT_HASH));
    let parse_jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='parse_document' AND payload->>'document_id'=$1",
    )
    .bind(document_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(parse_jobs, 1);
    cleanup(&pool, project_id).await;
}
