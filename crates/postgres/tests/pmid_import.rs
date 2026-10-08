#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use std::collections::HashSet;

use deepref_application::{RawAuthor, RawIdentifier, RawRecord};
use deepref_domain::{IdentifierScheme, ImportFormat};
use deepref_postgres::{
    ImportPersistRequest, PmidImportRequest, PmidOutcome, PmidRunCounts, complete_pmid_run,
    create_pmid_import, load_pmid_run, migrate, persist_import, pmid_run_counts, pmids_in_project,
    queued_pmids, save_pmid_outcomes, start_pmid_run,
};
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

fn pubmed_record(pmid: &str, title: &str) -> RawRecord {
    RawRecord {
        source_identifiers: vec![RawIdentifier::new(IdentifierScheme::Pmid, pmid)],
        title: Some(title.to_owned()),
        abstract_text: None,
        authors: vec![RawAuthor::literal("Wei Li")],
        publication_year: Some(2024),
        journal: Some("The Lancet".to_owned()),
        raw: json!({"source": "pubmed", "pmid": pmid}),
    }
}

#[tokio::test]
async fn a_pmid_run_saves_each_new_article_once_and_settles_with_its_counts() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'pmid import db test')")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
    // The project already holds PMID 222 from an earlier file import.
    persist_import(
        &pool,
        &ImportPersistRequest {
            project_id,
            source: "import:nbib".to_owned(),
            strategy: "file_import".to_owned(),
            format: ImportFormat::Nbib,
            idempotency_key: None,
            config: json!({}),
            metadata: json!({}),
        },
        &[pubmed_record("222", "Earlier record")],
    )
    .await
    .unwrap();

    let run = create_pmid_import(
        &pool,
        &PmidImportRequest {
            project_id,
            idempotency_key: Some("pmid-db-1".to_owned()),
            pmids: vec!["111".to_owned(), "222".to_owned(), "333".to_owned()],
        },
    )
    .await
    .unwrap();
    assert!(run.created);
    let queued = load_pmid_run(&pool, run.run_id).await.unwrap().unwrap();
    assert_eq!(queued.status, "queued");
    assert_eq!(queued.project_id, project_id);

    start_pmid_run(&pool, run.run_id).await.unwrap();
    assert_eq!(
        load_pmid_run(&pool, run.run_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "running"
    );
    let waiting = queued_pmids(&pool, run.run_id, 200).await.unwrap();
    assert_eq!(waiting, ["111", "222", "333"]);
    let held = pmids_in_project(&pool, project_id, &waiting).await.unwrap();
    assert_eq!(held, HashSet::from(["222".to_owned()]));

    let saved = save_pmid_outcomes(
        &pool,
        project_id,
        run.run_id,
        &[
            PmidOutcome::Imported {
                pmid: "111".to_owned(),
                title: Some("New record".to_owned()),
                record: pubmed_record("111", "New record"),
            },
            PmidOutcome::AlreadyInProject {
                pmid: "222".to_owned(),
                title: Some("Earlier record".to_owned()),
            },
            PmidOutcome::NotFound {
                pmid: "333".to_owned(),
            },
        ],
    )
    .await
    .unwrap();
    assert_eq!(saved, 1, "only the new article is saved as a record");
    assert!(
        queued_pmids(&pool, run.run_id, 200)
            .await
            .unwrap()
            .is_empty()
    );

    // Saving the same article again creates no second record.
    let again = save_pmid_outcomes(
        &pool,
        project_id,
        run.run_id,
        &[PmidOutcome::Imported {
            pmid: "111".to_owned(),
            title: None,
            record: pubmed_record("111", "New record"),
        }],
    )
    .await
    .unwrap();
    assert_eq!(again, 0);

    let mut tx = pool.begin().await.unwrap();
    let counts = pmid_run_counts(&mut tx, run.run_id).await.unwrap();
    assert_eq!(
        counts,
        PmidRunCounts {
            queued: 0,
            imported: 1,
            already_in_project: 1,
            not_found: 1,
            failed: 0,
        }
    );
    assert_eq!(counts.found(), 2);
    assert_eq!(counts.missed(), 1);
    complete_pmid_run(&mut tx, project_id, run.run_id, "completed", &counts)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let settled = sqlx::query(
        "SELECT status, fetched_count, failed_count, queued_count FROM acquisition_runs WHERE id=$1",
    )
    .bind(run.run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(settled.get::<String, _>("status"), "completed");
    assert_eq!(settled.get::<i32, _>("fetched_count"), 2);
    assert_eq!(settled.get::<i32, _>("failed_count"), 1);
    assert_eq!(settled.get::<i32, _>("queued_count"), 0);

    let saved_rows: Vec<(String, Option<Uuid>)> = sqlx::query_as(
        "SELECT source, acquisition_run_id FROM records
         WHERE project_id=$1 AND source='import:pmid'",
    )
    .bind(project_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(saved_rows, [("import:pmid".to_owned(), Some(run.run_id))]);
    let linked: Option<Uuid> = sqlx::query_scalar(
        "SELECT record_id FROM pmid_import_items WHERE acquisition_run_id=$1 AND pmid='111'",
    )
    .bind(run.run_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(linked.is_some(), "the item links the record it saved");
}
