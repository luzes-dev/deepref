use deepref_application::{RawAuthor, RawIdentifier, RawRecord};
use deepref_domain::{IdentifierScheme, ImportFormat};
use deepref_postgres::{AcquisitionError, ImportPersistRequest, migrate, persist_import};
use serde_json::json;
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .unwrap();
    migrate(&pool).await.unwrap();
    Some(pool)
}

#[tokio::test]
async fn generic_import_is_transactional_idempotent_and_report_free() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'postgres acquisition test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
    let record = RawRecord {
        source_identifiers: vec![RawIdentifier {
            scheme: IdentifierScheme::Doi,
            value: "DOI:10.5555/acquisition".to_owned(),
            normalized_value: "10.5555/acquisition".to_owned(),
        }],
        title: Some("An imported record".to_owned()),
        abstract_text: Some("A preserved abstract".to_owned()),
        authors: vec![RawAuthor::named(
            Some("Ana".to_owned()),
            Some("Müller".to_owned()),
        )],
        publication_year: Some(2024),
        journal: Some("Evidence Journal".to_owned()),
        raw: json!({"source_field": "kept"}),
    };
    let request = ImportPersistRequest {
        project_id,
        source: "import:ris".to_owned(),
        strategy: "file_import".to_owned(),
        format: ImportFormat::Ris,
        idempotency_key: Some("postgres-acquisition-1".to_owned()),
        config: json!({"content_sha256": "fixture"}),
        metadata: json!({"fixture": true}),
    };
    let first = persist_import(&pool, &request, std::slice::from_ref(&record))
        .await
        .unwrap();
    assert!(first.created);
    assert_eq!(first.records_created, 1);
    let second = persist_import(&pool, &request, &[record]).await.unwrap();
    assert!(!second.created);
    assert_eq!(second.run_id, first.run_id);
    assert_eq!(second.records_created, 0);

    let row = sqlx::query(
        "SELECT report_id,acquisition_run_id,authors,source_identifiers FROM records WHERE acquisition_run_id=$1",
    )
    .bind(first.run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(row.get::<Option<Uuid>, _>("report_id").is_none());
    assert_eq!(row.get::<Uuid, _>("acquisition_run_id"), first.run_id);
    assert_eq!(
        row.get::<serde_json::Value, _>("authors")[0]["family"],
        "Müller"
    );
    assert_eq!(
        row.get::<serde_json::Value, _>("source_identifiers")[0]["normalized_value"],
        "10.5555/acquisition"
    );
    let identifiers: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM record_identifiers WHERE record_id IN (SELECT id FROM records WHERE acquisition_run_id=$1)",
    )
    .bind(first.run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(identifiers, 1);

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

fn test_record() -> RawRecord {
    RawRecord {
        source_identifiers: vec![RawIdentifier {
            scheme: IdentifierScheme::Doi,
            value: "10.5555/concurrent".to_owned(),
            normalized_value: "10.5555/concurrent".to_owned(),
        }],
        title: Some("A concurrent import".to_owned()),
        abstract_text: None,
        authors: Vec::new(),
        publication_year: Some(2024),
        journal: Some("Evidence Journal".to_owned()),
        raw: json!({"fixture": "concurrency"}),
    }
}

fn test_request(project_id: Uuid, key: &str, config: serde_json::Value) -> ImportPersistRequest {
    ImportPersistRequest {
        project_id,
        source: "import:doi".to_owned(),
        strategy: "file_import".to_owned(),
        format: ImportFormat::Doi,
        idempotency_key: Some(key.to_owned()),
        config,
        metadata: json!({"fixture": true}),
    }
}

#[tokio::test]
async fn import_idempotency_same_key_same_payload_has_one_creator() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'concurrent import test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
    let request = test_request(project_id, "concurrent-import", json!({"digest": "same"}));
    let records_a = vec![test_record()];
    let records_b = vec![test_record()];
    let (first, second) = tokio::join!(
        persist_import(&pool, &request, &records_a),
        persist_import(&pool, &request, &records_b),
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_ne!(first.created, second.created);
    assert_eq!(first.run_id, second.run_id);
    assert_eq!(first.created as i32 + second.created as i32, 1);
    assert_eq!(first.records_created + second.records_created, 1);

    let acquisition_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM acquisition_runs WHERE project_id=$1 AND idempotency_key=$2",
    )
    .bind(project_id)
    .bind("concurrent-import")
    .fetch_one(&pool)
    .await
    .unwrap();
    let record_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM records WHERE project_id=$1 AND acquisition_run_id=$2",
    )
    .bind(project_id)
    .bind(first.run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(acquisition_count, 1);
    assert_eq!(record_count, 1);

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn import_idempotency_same_key_different_payload_conflicts_deterministically() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'conflicting import test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
    let first_request = test_request(project_id, "conflicting-import", json!({"digest": "a"}));
    let second_request = test_request(project_id, "conflicting-import", json!({"digest": "b"}));
    let records_a = vec![test_record()];
    let records_b = vec![test_record()];
    let (first, second) = tokio::join!(
        persist_import(&pool, &first_request, &records_a),
        persist_import(&pool, &second_request, &records_b),
    );
    let results = [first, second];
    assert_eq!(
        results
            .iter()
            .filter(|result| result.as_ref().is_ok_and(|value| value.created))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(AcquisitionError::IdempotencyConflict { .. })))
            .count(),
        1
    );

    let acquisition_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM acquisition_runs WHERE project_id=$1 AND idempotency_key=$2",
    )
    .bind(project_id)
    .bind("conflicting-import")
    .fetch_one(&pool)
    .await
    .unwrap();
    let record_count: i64 = sqlx::query_scalar("SELECT count(*) FROM records WHERE project_id=$1")
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(acquisition_count, 1);
    assert_eq!(record_count, 1);

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn imported_duplicate_source_records_are_preserved_without_reports() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'duplicate source record test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
    let request = test_request(
        project_id,
        "duplicate-source-records",
        json!({"digest": "dupes"}),
    );
    let records = vec![test_record(), test_record()];
    let result = persist_import(&pool, &request, &records).await.unwrap();
    assert_eq!(result.records_created, 2);
    let row = sqlx::query(
        "SELECT count(*) AS count, count(report_id) AS reports
         FROM records WHERE project_id=$1 AND acquisition_run_id=$2",
    )
    .bind(project_id)
    .bind(result.run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<i64, _>("count"), 2);
    assert_eq!(row.get::<i64, _>("reports"), 0);

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
}
