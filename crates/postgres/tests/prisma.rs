#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use deepref_postgres::{get_prisma_projection, migrate};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(12)
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

async fn delete_fixture(pool: &PgPool, project_id: Uuid, report_ids: &[Uuid]) {
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(pool)
        .await
        .expect("fixture project deletes");
    sqlx::query("DELETE FROM reports WHERE id=ANY($1)")
        .bind(report_ids)
        .execute(pool)
        .await
        .expect("fixture reports delete");
}

#[tokio::test]
async fn missing_project_is_none_and_empty_project_unused_reason_has_null_as_of() {
    let Some(pool) = database().await else {
        return;
    };
    assert!(
        get_prisma_projection(&pool, Uuid::new_v4())
            .await
            .expect("missing project projection reads")
            .is_none()
    );

    let project_id = Uuid::new_v4();
    let reason_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'empty prisma fixture')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("project inserts");
    sqlx::query(
        "INSERT INTO exclusion_reasons(id,project_id,code,label,stage)
         VALUES($1,$2,'unused_fixture_reason','Unused fixture reason','full_text')",
    )
    .bind(reason_id)
    .bind(project_id)
    .execute(&pool)
    .await
    .expect("unused reason inserts");

    let projection = get_prisma_projection(&pool, project_id)
        .await
        .expect("empty projection reads")
        .expect("project exists");
    assert!(projection.validate().is_ok());
    assert_eq!(projection.identified_records.get(), 0);
    assert_eq!(projection.screened_records.get(), 0);
    let unused_reason = projection
        .full_text_exclusions
        .iter()
        .find(|reason| reason.code == "unused_fixture_reason")
        .expect("unused fixture reason is exported");
    assert_eq!(unused_reason.count.get(), 0);
    assert!(projection.as_of.is_none());

    delete_fixture(&pool, project_id, &[]).await;
}

#[tokio::test]
async fn canonical_prisma_fixture_reconciles_external_decisions_and_deduplicates_grouping() {
    let Some(pool) = database().await else {
        return;
    };
    let project_id = Uuid::new_v4();
    let report_a = Uuid::new_v4();
    let report_b = Uuid::new_v4();
    let report_c = Uuid::new_v4();
    let report_d = Uuid::new_v4();
    let report_e = Uuid::new_v4();
    let report_f = Uuid::new_v4();
    let report_ids = [report_a, report_b, report_c, report_d, report_e, report_f];
    let record_a1 = Uuid::new_v4();
    let record_a2 = Uuid::new_v4();
    let record_b = Uuid::new_v4();
    let record_unresolved = Uuid::new_v4();
    let reason_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let study_id = Uuid::new_v4();

    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'canonical prisma fixture')")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("project inserts");
    for report_id in report_ids {
        sqlx::query("INSERT INTO reports(id,title) VALUES($1,$2)")
            .bind(report_id)
            .bind(format!("fixture report {report_id}"))
            .execute(&pool)
            .await
            .expect("report inserts");
    }
    sqlx::query(
        "INSERT INTO exclusion_reasons(id,project_id,code,label,stage)
         VALUES($1,$2,'fixture_reason','Fixture exclusion reason','full_text')",
    )
    .bind(reason_id)
    .bind(project_id)
    .execute(&pool)
    .await
    .expect("referenced reason inserts");
    for (record_id, report_id, source_key) in [
        (record_a1, Some(report_a), "a-1"),
        (record_a2, Some(report_a), "a-2"),
        (record_b, Some(report_b), "b-1"),
        (record_unresolved, None, "unresolved-1"),
    ] {
        sqlx::query(
            "INSERT INTO records(id,project_id,report_id,source,source_key,title)
             VALUES($1,$2,$3,'fixture',$4,'fixture record')",
        )
        .bind(record_id)
        .bind(project_id)
        .bind(report_id)
        .bind(source_key)
        .execute(&pool)
        .await
        .expect("record inserts");
    }
    sqlx::query(
        "INSERT INTO project_reports(project_id,report_id)
         SELECT $1, unnest($2::uuid[])",
    )
    .bind(project_id)
    .bind(report_ids.as_slice())
    .execute(&pool)
    .await
    .expect("project report inserts");
    sqlx::query(
        "INSERT INTO documents(
           id,project_id,report_id,source,status,object_key,content_hash,mime_type,byte_size,
           active_parser_version)
         VALUES($1,$2,$3,'upload','available',$4,$5,'application/pdf',1,'fixture-parser')",
    )
    .bind(document_id)
    .bind(project_id)
    .bind(report_b)
    .bind(format!("documents/{document_id}"))
    .bind("a".repeat(64))
    .execute(&pool)
    .await
    .expect("available document inserts");
    sqlx::query("INSERT INTO studies(id,project_id,title,design) VALUES($1,$2,'one study','rct')")
        .bind(study_id)
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("study inserts");
    sqlx::query(
        "INSERT INTO study_reports(project_id,study_id,report_id,relationship)
         VALUES($1,$2,$3,'report_of_study'),($1,$2,$4,'follow_up')",
    )
    .bind(project_id)
    .bind(study_id)
    .bind(report_a)
    .bind(report_e)
    .execute(&pool)
    .await
    .expect("study grouping inserts");
    sqlx::query(
        "INSERT INTO dedupe_proposals(
           id,project_id,record_id,candidate_report_id,proposal_kind,title_similarity,score)
         VALUES($1,$2,$3,$4,'fuzzy',0.9,0.9)",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(record_unresolved)
    .bind(report_d)
    .execute(&pool)
    .await
    .expect("pending dedupe proposal inserts");
    for (report_id, title_status, full_text_status, final_status, exclusion_reason_id) in [
        (report_a, "include", "include", "include", None),
        (report_b, "include", "unscreened", "pending_full_text", None),
        (report_c, "exclude", "not_required", "exclude", None),
        (
            report_d,
            "include",
            "not_required",
            "pending_full_text",
            None,
        ),
        (report_e, "include", "include", "include", None),
        (report_f, "include", "exclude", "exclude", Some(reason_id)),
    ] {
        sqlx::query(
            "INSERT INTO screening_state(
               project_id,report_id,title_abstract_status,full_text_status,
               full_text_exclusion_reason_id,final_status,revision)
             VALUES($1,$2,$3,$4,$5,$6,1)",
        )
        .bind(project_id)
        .bind(report_id)
        .bind(title_status)
        .bind(full_text_status)
        .bind(exclusion_reason_id)
        .bind(final_status)
        .execute(&pool)
        .await
        .expect("screening state inserts");
    }

    let projection = get_prisma_projection(&pool, project_id)
        .await
        .expect("canonical projection reads")
        .expect("project exists");
    assert!(projection.validate().is_ok());
    assert_eq!(projection.identified_records.get(), 4);
    assert_eq!(projection.linked_records.get(), 3);
    assert_eq!(projection.unresolved_records.get(), 1);
    assert_eq!(projection.duplicates_removed.get(), 1);
    assert_eq!(projection.source_canonical_reports.get(), 2);
    assert_eq!(projection.manually_created_reports.get(), 4);
    assert_eq!(projection.screened_records.get(), 6);
    assert_eq!(projection.title_abstract_excluded.get(), 1);
    assert_eq!(projection.title_abstract_pending.get(), 0);
    assert_eq!(projection.reports_sought.get(), 5);
    assert_eq!(projection.reports_not_retrieved.get(), 1);
    assert_eq!(projection.full_text_assessed.get(), 4);
    assert_eq!(projection.full_text_pending.get(), 1);
    assert_eq!(projection.full_text_included.get(), 2);
    assert_eq!(projection.full_text_excluded.get(), 1);
    assert_eq!(projection.included_reports_not_grouped.get(), 0);
    assert_eq!(projection.included_studies.get(), 1);
    assert_eq!(projection.pending_dedupe_proposals.get(), 1);
    assert_eq!(projection.screening_high_watermark.get(), 1);
    let fixture_reason = projection
        .full_text_exclusions
        .iter()
        .find(|reason| reason.code == "fixture_reason")
        .expect("referenced fixture reason is exported");
    assert_eq!(fixture_reason.count.get(), 1);
    assert_eq!(
        projection
            .grouped_reports()
            .expect("grouping equation")
            .get(),
        2
    );
    assert_eq!(
        projection
            .full_text_exclusions
            .iter()
            .map(|reason| reason.count.get())
            .sum::<u64>(),
        projection.full_text_excluded.get()
    );
    let before_reason_update = projection.as_of.expect("populated projection has as_of");

    // Reports A and E's full-text includes are deliberately external/no-
    // document decisions and map to one study. Report D has neither document
    // nor decision and remains the sole not-retrieved report. Report B proves
    // an available document counts as assessed while still pending.
    assert_eq!(projection.reports_not_retrieved.get(), 1);
    assert_eq!(projection.full_text_assessed.get(), 4);

    sqlx::query("SELECT pg_sleep(0.01)")
        .execute(&pool)
        .await
        .expect("database clock advances");
    sqlx::query("UPDATE exclusion_reasons SET label='updated fixture reason' WHERE id=$1")
        .bind(reason_id)
        .execute(&pool)
        .await
        .expect("referenced reason updates");
    let after_reason_update = get_prisma_projection(&pool, project_id)
        .await
        .expect("fresh projection reads")
        .expect("project exists")
        .as_of
        .expect("populated projection remains timestamped");
    assert!(
        after_reason_update > before_reason_update,
        "referenced reason edits must advance as_of"
    );

    delete_fixture(&pool, project_id, &report_ids).await;
}
