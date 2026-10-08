#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]

use deepref_postgres::list_ungrouped_included_reports;
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .ok()?;
    deepref_postgres::migrate(&pool).await.ok()?;
    Some(pool)
}

async fn add_report(pool: &PgPool, project: Uuid, title: &str) -> Uuid {
    let report = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO reports (id,title,publication_year,journal) VALUES ($1,$2,2001,'Stroke')",
    )
    .bind(report)
    .bind(title)
    .execute(pool)
    .await
    .expect("report inserts");
    sqlx::query(
        "INSERT INTO project_reports (project_id,report_id,lifecycle_status) VALUES ($1,$2,'included')",
    )
    .bind(project)
    .bind(report)
    .execute(pool)
    .await
    .expect("project report inserts");
    report
}

#[tokio::test]
async fn included_reports_without_a_study_are_listed_until_a_study_takes_them() {
    let Some(pool) = database().await else {
        return;
    };
    let project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'ungrouped test')")
        .bind(project)
        .execute(&pool)
        .await
        .expect("project inserts");
    let reason = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO exclusion_reasons (id,project_id,code,label,stage)
         VALUES ($1,$2,'ungrouped_fixture_reason','Fixture reason','full_text')",
    )
    .bind(reason)
    .bind(project)
    .execute(&pool)
    .await
    .expect("exclusion reason inserts");

    let ungrouped = add_report(&pool, project, "Included and ungrouped").await;
    // DOIs are unique across the whole database, so each run gets its own.
    let doi = format!("10.1000/ungrouped-{ungrouped}");
    let grouped = add_report(&pool, project, "Included and grouped").await;
    let excluded = add_report(&pool, project, "Excluded at full text").await;
    let _unscreened = add_report(&pool, project, "Never screened").await;

    for (report, full_text, reason_id) in [
        (ungrouped, "include", None),
        (grouped, "include", None),
        (excluded, "exclude", Some(reason)),
    ] {
        sqlx::query(
            "INSERT INTO screening_state
             (project_id,report_id,title_abstract_status,full_text_status,final_status,full_text_exclusion_reason_id)
             VALUES ($1,$2,'include',$3,$4,$5)",
        )
        .bind(project)
        .bind(report)
        .bind(full_text)
        .bind(if full_text == "include" { "include" } else { "exclude" })
        .bind(reason_id)
        .execute(&pool)
        .await
        .expect("screening state inserts");
    }
    sqlx::query(
        "INSERT INTO report_identifiers (id,report_id,scheme,value,normalized_value)
         VALUES (gen_random_uuid(),$1,'doi',$2,$2)",
    )
    .bind(ungrouped)
    .bind(&doi)
    .execute(&pool)
    .await
    .expect("identifier inserts");

    let study = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO studies
         (id,project_id,title,design_context,study_revision,updated_by_actor_kind,updated_by_actor_id)
         VALUES ($1,$2,'Grouped study','{}'::jsonb,0,'user','fixture')",
    )
    .bind(study)
    .bind(project)
    .execute(&pool)
    .await
    .expect("study inserts");
    sqlx::query(
        "INSERT INTO study_reports (project_id,study_id,report_id,relationship)
         VALUES ($1,$2,$3,'report_of_study')",
    )
    .bind(project)
    .bind(study)
    .bind(grouped)
    .execute(&pool)
    .await
    .expect("study report inserts");

    let listed = list_ungrouped_included_reports(&pool, project)
        .await
        .expect("ungrouped reports list");
    assert_eq!(listed.len(), 1, "only the included report without a study");
    assert_eq!(listed[0].report_id, ungrouped);
    assert_eq!(listed[0].doi.as_deref(), Some(doi.as_str()));
    assert_eq!(listed[0].publication_year, Some(2001));
    assert_eq!(listed[0].journal.as_deref(), Some("Stroke"));

    sqlx::query(
        "INSERT INTO study_reports (project_id,study_id,report_id,relationship)
         VALUES ($1,$2,$3,'report_of_study')",
    )
    .bind(project)
    .bind(study)
    .bind(ungrouped)
    .execute(&pool)
    .await
    .expect("second study report inserts");
    let after = list_ungrouped_included_reports(&pool, project)
        .await
        .expect("ungrouped reports list");
    assert!(after.is_empty(), "a grouped report leaves the list");
}
