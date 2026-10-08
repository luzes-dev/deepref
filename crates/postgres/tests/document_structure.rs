#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
use deepref_documents::ParsedDocument;
use deepref_postgres::{
    DocumentStructure, NewDocument, NewDocumentReference, NewDocumentSection, create_document,
    list_document_references, list_document_sections, migrate,
    persist_parsed_document_with_structure,
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .ok()?;
    migrate(&pool).await.ok()?;
    Some(pool)
}

#[tokio::test]
async fn structure_is_versioned_and_listed_for_the_active_parser_version() {
    let Some(pool) = database().await else { return };
    let project_id = Uuid::new_v4();
    let report_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'structure fixture')")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO reports (id,title,abstract_text) VALUES ($1,'R','A')")
        .bind(report_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project_id)
        .bind(report_id)
        .execute(&pool)
        .await
        .unwrap();
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
            original_filename: Some("s.pdf"),
            external_url: None,
            mime_type: "application/pdf",
            byte_size: 1,
            content_hash: Some("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"),
            object_key: Some(&object_key),
            actor_kind: "user",
            actor_id: "structure-tester",
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let parsed = ParsedDocument {
        pages: vec![],
        blocks: vec![],
        ocr_required: false,
    };
    let structure = DocumentStructure {
        sections: vec![NewDocumentSection {
            number: Some("1".to_owned()),
            title: "Introduction".to_owned(),
            depth: 1,
            path: vec!["Introduction".to_owned()],
            source: "grobid".to_owned(),
        }],
        references: vec![NewDocumentReference {
            raw: "Doe J. Aspirin. Stroke. 2019.".to_owned(),
            title: Some("Aspirin".to_owned()),
            authors: vec!["Jane Doe".to_owned()],
            year: Some(2019),
            venue: Some("Stroke".to_owned()),
            doi: Some("10.1000/x".to_owned()),
            source: "grobid".to_owned(),
        }],
        ..Default::default()
    };
    persist_parsed_document_with_structure(
        &pool,
        document_id,
        &parsed,
        "p-v1+grobid-0.8.2",
        &structure,
    )
    .await
    .unwrap();
    let references = list_document_references(&pool, project_id, report_id, document_id)
        .await
        .unwrap();
    assert_eq!(references.len(), 1);
    assert_eq!(references[0].authors, ["Jane Doe"]);
    assert_eq!(references[0].year, Some(2019));
    let sections = list_document_sections(&pool, project_id, report_id, document_id)
        .await
        .unwrap();
    assert_eq!(sections[0].path, ["Introduction"]);

    // A later native-only parse becomes active and hides the enriched rows.
    persist_parsed_document_with_structure(
        &pool,
        document_id,
        &parsed,
        "p-v1",
        &DocumentStructure::default(),
    )
    .await
    .unwrap();
    assert!(
        list_document_references(&pool, project_id, report_id, document_id)
            .await
            .unwrap()
            .is_empty()
    );

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}
