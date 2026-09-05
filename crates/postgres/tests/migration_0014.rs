#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use sqlx::{AssertSqlSafe, Connection, Executor, PgConnection, Row};
use uuid::Uuid;

const MIGRATIONS_THROUGH_PR8: [&str; 13] = [
    include_str!("../migrations/0001_initial.sql"),
    include_str!("../migrations/0002_metrics.sql"),
    include_str!("../migrations/0003_outbox_claims.sql"),
    include_str!("../migrations/0004_ingestion_durability.sql"),
    include_str!("../migrations/0005_domain_projection.sql"),
    include_str!("../migrations/0006_evidence_workspace.sql"),
    include_str!("../migrations/0007_evidence_identity.sql"),
    include_str!("../migrations/0008_infrastructure_collapse.sql"),
    include_str!("../migrations/0009_acquisition_runs.sql"),
    include_str!("../migrations/0010_deduplication.sql"),
    include_str!("../migrations/0011_protocol_versions.sql"),
    include_str!("../migrations/0012_title_abstract_screening.sql"),
    include_str!("../migrations/0013_documents_full_text.sql"),
];

#[tokio::test]
async fn upgrades_legacy_studies_and_enforces_project_scoped_truth() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        return;
    };
    let mut connection = PgConnection::connect(&url)
        .await
        .expect("database connects");
    let schema = format!("pr9_upgrade_{}", Uuid::new_v4().simple());
    connection
        .execute(AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .await
        .expect("isolated schema creates");
    connection
        .execute(AssertSqlSafe(format!("SET search_path TO {schema},public")))
        .await
        .expect("search path changes");
    for migration in MIGRATIONS_THROUGH_PR8 {
        connection
            .execute(migration)
            .await
            .expect("migration applies");
    }

    let project_a = Uuid::new_v4();
    let project_b = Uuid::new_v4();
    let report = Uuid::new_v4();
    let study_a = Uuid::new_v4();
    let study_b = Uuid::new_v4();
    let study_blank = Uuid::new_v4();
    let study_long = Uuid::new_v4();
    let study_unknown = Uuid::new_v4();
    sqlx::query("INSERT INTO reports(id,title) VALUES($1,'shared report')")
        .bind(report)
        .execute(&mut connection)
        .await
        .unwrap();
    for (project, name) in [(project_a, "project a"), (project_b, "project b")] {
        sqlx::query("INSERT INTO projects(id,name) VALUES($1,$2)")
            .bind(project)
            .bind(name)
            .execute(&mut connection)
            .await
            .unwrap();
        sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
            .bind(project)
            .bind(report)
            .execute(&mut connection)
            .await
            .unwrap();
    }
    sqlx::query(
        "INSERT INTO studies(id,project_id,title,design) VALUES($1,$2,'legacy trial','rct'),($3,$2,'   ','cohort'),($4,$2,$5,'legacy_unknown')",
    )
    .bind(study_a)
    .bind(project_a)
    .bind(study_blank)
    .bind(study_long)
    .bind("x".repeat(250))
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO studies(id,project_id,title,design) VALUES($1,$2,NULL,NULL),($3,$2,'unknown design','legacy_unknown')",
    )
    .bind(study_b)
    .bind(project_b)
    .bind(study_unknown)
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query("INSERT INTO study_reports(study_id,report_id,relationship) VALUES($1,$2,'report_of_study')")
        .bind(study_a)
        .bind(report)
        .execute(&mut connection)
        .await
        .unwrap();

    connection
        .execute(include_str!("../migrations/0014_studies_appraisals.sql"))
        .await
        .expect("PR9 migration applies");

    let migrated = sqlx::query(
        "SELECT project_id, design, title, study_revision, updated_by_actor_kind
         FROM studies WHERE id=$1",
    )
    .bind(study_a)
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(migrated.get::<Uuid, _>("project_id"), project_a);
    assert_eq!(migrated.get::<String, _>("design"), "rct");
    assert_eq!(migrated.get::<String, _>("title"), "legacy trial");
    assert_eq!(migrated.get::<i64, _>("study_revision"), 0);
    assert_eq!(migrated.get::<String, _>("updated_by_actor_kind"), "system");
    let blank = sqlx::query("SELECT title, design FROM studies WHERE id=$1")
        .bind(study_blank)
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(
        blank.get::<String, _>("title"),
        format!("Study {study_blank}")
    );
    assert_eq!(
        blank.get::<Option<String>, _>("design"),
        Some("cohort".to_owned())
    );
    let long = sqlx::query_scalar::<_, String>("SELECT title FROM studies WHERE id=$1")
        .bind(study_long)
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(long.chars().count(), 200);
    assert_eq!(
        sqlx::query_scalar::<_, Option<String>>("SELECT design FROM studies WHERE id=$1")
            .bind(study_unknown)
            .fetch_one(&mut connection)
            .await
            .unwrap(),
        None
    );
    let null_legacy = sqlx::query("SELECT title, design FROM studies WHERE id=$1")
        .bind(study_b)
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(
        null_legacy.get::<String, _>("title"),
        format!("Study {study_b}")
    );
    assert_eq!(null_legacy.get::<Option<String>, _>("design"), None);
    assert_eq!(
        sqlx::query_scalar::<_, Uuid>("SELECT project_id FROM study_reports WHERE study_id=$1")
            .bind(study_a)
            .fetch_one(&mut connection)
            .await
            .unwrap(),
        project_a
    );

    sqlx::query("INSERT INTO study_reports(project_id,study_id,report_id) VALUES($1,$2,$3)")
        .bind(project_b)
        .bind(study_b)
        .bind(report)
        .execute(&mut connection)
        .await
        .expect("shared report may belong to a study in another project");
    let duplicate =
        sqlx::query("INSERT INTO study_reports(project_id,study_id,report_id) VALUES($1,$2,$3)")
            .bind(project_a)
            .bind(study_a)
            .bind(report)
            .execute(&mut connection)
            .await;
    assert!(
        duplicate.is_err(),
        "one report must have one study per project"
    );

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_a)
        .execute(&mut connection)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM studies WHERE project_id=$1")
            .bind(project_a)
            .fetch_one(&mut connection)
            .await
            .unwrap(),
        0
    );

    connection
        .execute("SET search_path TO public")
        .await
        .unwrap();
    connection
        .execute(AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .await
        .unwrap();
}
