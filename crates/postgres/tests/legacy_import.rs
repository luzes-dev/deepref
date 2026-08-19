use anyhow::{Result, ensure};
use deepref_postgres::{import_legacy, migrate};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(url) => url,
        Err(_) => return None,
    };
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .unwrap_or_else(|error| {
            panic!("DATABASE_URL is set but PostgreSQL is unavailable: {error}")
        });
    migrate(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to apply PostgreSQL migrations: {error}"));
    Some(pool)
}

#[tokio::test]
async fn legacy_import_round_trips_and_converges() {
    let Some(pool) = database().await else {
        return;
    };

    let project_id = Uuid::new_v4();
    let ingestion_one = Uuid::new_v4();
    let ingestion_two = Uuid::new_v4();
    let stale_report_id = Uuid::new_v4();
    let doi_one = format!("10.5555/deepref-import-{}-one", Uuid::new_v4());
    let doi_two = format!("10.5555/deepref-import-{}-two", Uuid::new_v4());
    let dois = vec![doi_one.clone(), doi_two.clone()];

    let result = seed_and_assert(
        &pool,
        project_id,
        ingestion_one,
        ingestion_two,
        stale_report_id,
        &doi_one,
        &doi_two,
    )
    .await;

    let cleanup_result = cleanup_fixture(&pool, project_id, &dois, stale_report_id).await;
    cleanup_result.expect("unique legacy import fixture should be removable");
    result.expect("legacy importer should preserve and converge the fixture");
}

async fn seed_and_assert(
    pool: &PgPool,
    project_id: Uuid,
    ingestion_one: Uuid,
    ingestion_two: Uuid,
    stale_report_id: Uuid,
    doi_one: &str,
    doi_two: &str,
) -> Result<()> {
    let stale_identifier_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'legacy import test')")
        .bind(project_id)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO reports (id,title,abstract_text,publication_year,journal,url,raw) \
         VALUES ($1,'Stale title','Stale abstract',1999,'Stale journal','https://stale.example', $2)",
    )
    .bind(stale_report_id)
    .bind(serde_json::json!({"fixture": "stale"}))
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO report_identifiers (id,report_id,scheme,value,normalized_value) \
         VALUES ($1,$2,'doi',$3,$4)",
    )
    .bind(stale_identifier_id)
    .bind(stale_report_id)
    .bind(doi_one)
    .bind(doi_one.to_ascii_lowercase())
    .execute(pool)
    .await?;

    for (doi, title, abstract_text, issued_year, published_year, journal, url, raw) in [
        (
            doi_one,
            "Repaired source one",
            Some("Repaired abstract one"),
            Some(2024),
            Some(2025),
            Some("Repaired journal"),
            Some("https://example.test/one"),
            serde_json::json!({"fixture": "one", "revision": 2}),
        ),
        (
            doi_two,
            "Legacy source two",
            Some("Legacy abstract two"),
            Some(2023),
            None,
            Some("Legacy journal"),
            Some("https://example.test/two"),
            serde_json::json!({"fixture": "two"}),
        ),
    ] {
        sqlx::query(
            "INSERT INTO works (canonical_doi,title,abstract_text,issued_year,published_year,container_title,url,fetch_status,raw) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,'fetched',$8)",
        )
        .bind(doi)
        .bind(title)
        .bind(abstract_text)
        .bind(issued_year)
        .bind(published_year)
        .bind(journal)
        .bind(url)
        .bind(raw)
        .execute(pool)
        .await?;
    }

    let work_event_one = Uuid::new_v4();
    let work_event_two = Uuid::new_v4();
    for (
        ingestion_id,
        doi,
        max_depth,
        seed_count,
        queued_count,
        fetched_count,
        failed_count,
        metadata_provider,
        citation_provider,
    ) in [
        (
            ingestion_one,
            doi_one,
            4,
            7,
            8,
            9,
            10,
            "fixture-metadata-one",
            "fixture-citations-one",
        ),
        (
            ingestion_two,
            doi_two,
            5,
            11,
            12,
            13,
            14,
            "fixture-metadata-two",
            "fixture-citations-two",
        ),
    ] {
        sqlx::query(
            "INSERT INTO ingestions (id,project_id,status,max_depth,seed_count,queued_count,fetched_count,failed_count,metadata_provider,citation_provider,created_at,started_at,completed_at) \
             VALUES ($1,$2,'completed',$3,$4,$5,$6,$7,$8,$9,now(),now(),now())",
        )
        .bind(ingestion_id)
        .bind(project_id)
        .bind(max_depth)
        .bind(seed_count)
        .bind(queued_count)
        .bind(fetched_count)
        .bind(failed_count)
        .bind(metadata_provider)
        .bind(citation_provider)
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO project_works (project_id,canonical_doi,first_seen_ingestion_id,seed,min_depth) \
             VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(project_id)
        .bind(doi)
        .bind(ingestion_id)
        .bind(ingestion_id == ingestion_one)
        .bind(if ingestion_id == ingestion_one { 0 } else { 1 })
        .execute(pool)
        .await?;
    }
    sqlx::query(
        "INSERT INTO ingestion_items (ingestion_id,project_id,canonical_doi,depth,parent_doi,status,attempts,last_error,work_event_id,fetched_at) \
         VALUES ($1,$2,$3,1,'10.5555/parent-one','fetched',3,NULL,$4,now()),\
                ($5,$2,$6,2,NULL,'failed',4,'legacy fetch failed',$7,NULL)",
    )
    .bind(ingestion_one)
    .bind(project_id)
    .bind(doi_one)
    .bind(work_event_one)
    .bind(ingestion_two)
    .bind(doi_two)
    .bind(work_event_two)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO legacy_citations (project_id,source_doi,target_doi,source,first_seen_ingestion_id) \
         VALUES ($1,$2,$3,'legacy-test',$4)",
    )
    .bind(project_id)
    .bind(doi_one)
    .bind(doi_two)
    .bind(ingestion_one)
    .execute(pool)
    .await?;

    let first = import_legacy(pool).await?;
    ensure!(
        first.reports_created == 1 && first.reports_reused == 1,
        "expected one new report and one repaired mapping: {first:?}"
    );
    ensure!(
        first.report_identifiers_created == 1,
        "expected one new identifier: {first:?}"
    );
    ensure!(
        first.records_created == 4,
        "expected four records: {first:?}"
    );
    ensure!(
        first.provenance_rows_created == 2,
        "expected two provenance rows: {first:?}"
    );
    ensure!(
        first.project_reports_created == 2,
        "expected two memberships: {first:?}"
    );
    ensure!(
        first.acquisition_runs_created == 2,
        "expected two acquisition runs: {first:?}"
    );
    ensure!(
        first.citations_created == 1,
        "expected one citation: {first:?}"
    );

    let repaired_report = sqlx::query(
        "SELECT ri.report_id,r.title,r.abstract_text,r.publication_year,r.journal,r.url,r.raw \
         FROM report_identifiers ri JOIN reports r ON r.id=ri.report_id \
         WHERE ri.scheme='doi' AND ri.normalized_value=$1",
    )
    .bind(doi_one.to_ascii_lowercase())
    .fetch_one(pool)
    .await?;
    ensure!(
        repaired_report.get::<Uuid, _>("report_id") == stale_report_id,
        "existing DOI mapping must preserve its report UUID"
    );
    ensure!(
        repaired_report.get::<String, _>("title") == "Repaired source one"
            && repaired_report.get::<String, _>("abstract_text") == "Repaired abstract one"
            && repaired_report.get::<i32, _>("publication_year") == 2025
            && repaired_report.get::<String, _>("journal") == "Repaired journal"
            && repaired_report.get::<String, _>("url") == "https://example.test/one"
            && repaired_report.get::<serde_json::Value, _>("raw")
                == serde_json::json!({"fixture": "one", "revision": 2}),
        "existing report metadata was not repaired from works"
    );

    let runs = sqlx::query(
        "SELECT legacy_ingestion_id,project_id,status,max_depth,seed_count,queued_count,fetched_count,failed_count,metadata_provider,citation_provider \
         FROM acquisition_runs WHERE project_id=$1 ORDER BY legacy_ingestion_id",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    ensure!(runs.len() == 2, "expected exactly two acquisition runs");
    let run_one = runs
        .iter()
        .find(|row| row.get::<Uuid, _>("legacy_ingestion_id") == ingestion_one)
        .expect("first acquisition run should be present");
    let run_two = runs
        .iter()
        .find(|row| row.get::<Uuid, _>("legacy_ingestion_id") == ingestion_two)
        .expect("second acquisition run should be present");
    for (row, ingestion_id, max_depth, seed_count, queued_count, fetched_count, failed_count) in [
        (run_one, ingestion_one, 4, 7, 8, 9, 10),
        (run_two, ingestion_two, 5, 11, 12, 13, 14),
    ] {
        ensure!(
            row.get::<Uuid, _>("legacy_ingestion_id") == ingestion_id
                && row.get::<Uuid, _>("project_id") == project_id
                && row.get::<String, _>("status") == "completed"
                && row.get::<i32, _>("max_depth") == max_depth
                && row.get::<i32, _>("seed_count") == seed_count
                && row.get::<i32, _>("queued_count") == queued_count
                && row.get::<i32, _>("fetched_count") == fetched_count
                && row.get::<i32, _>("failed_count") == failed_count,
            "acquisition run provenance was not preserved for {ingestion_id}"
        );
    }
    ensure!(
        run_one.get::<String, _>("metadata_provider") == "fixture-metadata-one"
            && run_one.get::<String, _>("citation_provider") == "fixture-citations-one"
            && run_two.get::<String, _>("metadata_provider") == "fixture-metadata-two"
            && run_two.get::<String, _>("citation_provider") == "fixture-citations-two",
        "acquisition providers were not preserved"
    );

    let project_work_record: Uuid = sqlx::query_scalar(
        "SELECT acquisition_run_id FROM records WHERE project_id=$1 AND source='legacy_project_works' AND source_key=$2",
    )
    .bind(project_id)
    .bind(doi_one)
    .fetch_one(pool)
    .await?;
    ensure!(
        project_work_record == ingestion_one,
        "project work record lost acquisition-run linkage"
    );
    let item_record: Uuid = sqlx::query_scalar(
        "SELECT acquisition_run_id FROM records WHERE project_id=$1 AND source='legacy_ingestion_item' AND source_key=$2",
    )
    .bind(project_id)
    .bind(format!("{ingestion_two}:{doi_two}"))
    .fetch_one(pool)
    .await?;
    ensure!(
        item_record == ingestion_two,
        "ingestion item record lost acquisition-run linkage"
    );

    let provenance_one = sqlx::query(
        "SELECT acquisition_run_id,canonical_doi,depth,parent_doi,status,attempts,queued_at,fetched_at,last_error,work_event_id \
         FROM record_provenance WHERE acquisition_run_id=$1 AND canonical_doi=$2",
    )
    .bind(ingestion_one)
    .bind(doi_one)
    .fetch_one(pool)
    .await?;
    ensure!(
        provenance_one.get::<Uuid, _>("acquisition_run_id") == ingestion_one
            && provenance_one.get::<String, _>("canonical_doi") == doi_one
            && provenance_one.get::<i32, _>("depth") == 1
            && provenance_one
                .get::<Option<String>, _>("parent_doi")
                .as_deref()
                == Some("10.5555/parent-one")
            && provenance_one.get::<String, _>("status") == "fetched"
            && provenance_one.get::<i32, _>("attempts") == 3
            && provenance_one
                .get::<Option<String>, _>("last_error")
                .is_none()
            && provenance_one.get::<Option<Uuid>, _>("work_event_id") == Some(work_event_one)
            && provenance_one
                .get::<Option<chrono::DateTime<chrono::Utc>>, _>("fetched_at")
                .is_some(),
        "first ingestion item provenance was not preserved exactly"
    );
    let provenance_two = sqlx::query(
        "SELECT acquisition_run_id,canonical_doi,depth,parent_doi,status,attempts,queued_at,fetched_at,last_error,work_event_id \
         FROM record_provenance WHERE acquisition_run_id=$1 AND canonical_doi=$2",
    )
    .bind(ingestion_two)
    .bind(doi_two)
    .fetch_one(pool)
    .await?;
    ensure!(
        provenance_two.get::<Uuid, _>("acquisition_run_id") == ingestion_two
            && provenance_two.get::<String, _>("canonical_doi") == doi_two
            && provenance_two.get::<i32, _>("depth") == 2
            && provenance_two
                .get::<Option<String>, _>("parent_doi")
                .is_none()
            && provenance_two.get::<String, _>("status") == "failed"
            && provenance_two.get::<i32, _>("attempts") == 4
            && provenance_two
                .get::<Option<String>, _>("last_error")
                .as_deref()
                == Some("legacy fetch failed")
            && provenance_two.get::<Option<Uuid>, _>("work_event_id") == Some(work_event_two)
            && provenance_two
                .get::<Option<chrono::DateTime<chrono::Utc>>, _>("fetched_at")
                .is_none(),
        "second ingestion item provenance was not preserved exactly"
    );

    let second = import_legacy(pool).await?;
    ensure!(
        second.reports_created == 0 && second.reports_reused == 2,
        "reports were not reused on rerun: {second:?}"
    );
    ensure!(
        second.report_identifiers_created == 0,
        "identifiers duplicated: {second:?}"
    );
    ensure!(
        second.records_created == 0,
        "records duplicated: {second:?}"
    );
    ensure!(
        second.provenance_rows_created == 0,
        "provenance duplicated: {second:?}"
    );
    ensure!(
        second.project_reports_created == 0,
        "memberships duplicated: {second:?}"
    );
    ensure!(
        second.acquisition_runs_created == 0 && second.acquisition_runs_reused == 2,
        "acquisition runs duplicated: {second:?}"
    );
    ensure!(
        second.citations_created == 0 && second.citations_repaired == 1,
        "citation should be repaired on rerun: {second:?}"
    );

    let report_count: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM report_identifiers WHERE scheme='doi' AND normalized_value = ANY($1)",
    )
    .bind([doi_one.to_ascii_lowercase(), doi_two.to_ascii_lowercase()])
    .fetch_one(pool)
    .await?;
    ensure!(report_count == 2, "expected two DOI report identifiers");
    let (records, project_work_records, item_records, memberships, runs, provenance, citations):
        (i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
         (SELECT count(*) FROM records WHERE project_id=$1),\
         (SELECT count(*) FROM records WHERE project_id=$1 AND source='legacy_project_works'),\
         (SELECT count(*) FROM records WHERE project_id=$1 AND source='legacy_ingestion_item'),\
         (SELECT count(*) FROM project_reports WHERE project_id=$1),\
         (SELECT count(*) FROM acquisition_runs WHERE project_id=$1),\
         (SELECT count(*) FROM record_provenance rp JOIN acquisition_runs ar ON ar.id=rp.acquisition_run_id WHERE ar.project_id=$1),\
         (SELECT count(*) FROM citations WHERE project_id=$1)",
    )
    .bind(project_id)
    .fetch_one(pool)
    .await?;
    ensure!(
        (
            records,
            project_work_records,
            item_records,
            memberships,
            runs,
            provenance,
            citations,
        ) == (4, 2, 2, 2, 2, 2, 1),
        "unexpected imported row counts: {records}, {project_work_records}, {item_records}, {memberships}, {runs}, {provenance}, {citations}"
    );

    let citation = sqlx::query(
        "SELECT source_report_id,target_report_id,legacy_source_doi,legacy_target_doi,source,first_seen_ingestion_id \
         FROM citations WHERE project_id=$1",
    )
    .bind(project_id)
    .fetch_one(pool)
    .await?;
    ensure!(
        citation.get::<Uuid, _>("source_report_id") != citation.get::<Uuid, _>("target_report_id"),
        "citation endpoints should be distinct"
    );
    ensure!(
        citation.get::<String, _>("legacy_source_doi") == doi_one,
        "source DOI was not preserved"
    );
    ensure!(
        citation.get::<String, _>("legacy_target_doi") == doi_two,
        "target DOI was not preserved"
    );
    ensure!(
        citation.get::<String, _>("source") == "legacy-test",
        "citation source was not preserved"
    );
    ensure!(
        citation.get::<Uuid, _>("first_seen_ingestion_id") == ingestion_one,
        "citation ingestion provenance was not preserved"
    );
    Ok(())
}

async fn cleanup_fixture(
    pool: &PgPool,
    project_id: Uuid,
    dois: &[String],
    stale_report_id: Uuid,
) -> Result<()> {
    let normalized_dois: Vec<String> = dois.iter().map(|doi| doi.to_ascii_lowercase()).collect();
    let mut report_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT report_id FROM report_identifiers WHERE scheme='doi' AND normalized_value = ANY($1)",
    )
    .bind(&normalized_dois)
    .fetch_all(pool)
    .await?;
    report_ids.push(stale_report_id);
    report_ids.sort_unstable();
    report_ids.dedup();

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM works WHERE canonical_doi = ANY($1)")
        .bind(dois.to_vec())
        .execute(pool)
        .await?;
    sqlx::query(
        "DELETE FROM report_identifiers WHERE report_id = ANY($1) AND scheme='doi' AND normalized_value = ANY($2)",
    )
    .bind(&report_ids)
    .bind(&normalized_dois)
    .execute(pool)
    .await?;
    sqlx::query("DELETE FROM reports WHERE id = ANY($1)")
        .bind(&report_ids)
        .execute(pool)
        .await?;
    Ok(())
}
