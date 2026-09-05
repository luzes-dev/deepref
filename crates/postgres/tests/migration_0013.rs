#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use sqlx::{AssertSqlSafe, Connection, Executor, PgConnection, Row};
use uuid::Uuid;

const PRE_PR8_MIGRATIONS: [&str; 12] = [
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
];

#[tokio::test]
async fn upgrades_legacy_documents_blocks_and_reason_catalog_truthfully() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        return;
    };
    let mut connection = PgConnection::connect(&url)
        .await
        .expect("database connects");
    let schema = format!("pr8_upgrade_{}", Uuid::new_v4().simple());
    let create_schema = format!("CREATE SCHEMA {schema}");
    sqlx::query(AssertSqlSafe(create_schema))
        .execute(&mut connection)
        .await
        .expect("isolated schema creates");
    let set_search_path = format!("SET search_path TO {schema},public");
    sqlx::query(AssertSqlSafe(set_search_path))
        .execute(&mut connection)
        .await
        .expect("search path changes");
    for migration in PRE_PR8_MIGRATIONS {
        connection
            .execute(migration)
            .await
            .expect("pre-PR8 migration applies");
    }

    let project_id = Uuid::new_v4();
    let report_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'legacy project')")
        .bind(project_id)
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("INSERT INTO reports(id,title) VALUES($1,'legacy report')")
        .bind(report_id)
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
        .bind(project_id)
        .bind(report_id)
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO documents(id,report_id,object_key,content_hash,mime_type,byte_size,parser_version,parse_status)
         VALUES($1,$2,$3,$4,'application/pdf',42,'legacy-v1','ocr_required')",
    )
    .bind(document_id)
    .bind(report_id)
    .bind(format!("documents/{}", Uuid::new_v4()))
    .bind("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO document_blocks(id,document_id,parser_version,page_number,kind,ordinal,text,content_hash)
         VALUES($1,$2,'legacy-v1',1,'text',0,'legacy evidence',$3)",
    )
    .bind(Uuid::new_v4())
    .bind(document_id)
    .bind("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
    .execute(&mut connection)
    .await
    .unwrap();

    connection
        .execute(include_str!("../migrations/0013_documents_full_text.sql"))
        .await
        .expect("PR8 migration applies to legacy rows");

    let document = sqlx::query(
        "SELECT project_id,status,active_parser_version,ocr_required FROM documents WHERE id=$1",
    )
    .bind(document_id)
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(document.get::<Uuid, _>("project_id"), project_id);
    assert_eq!(document.get::<String, _>("status"), "available");
    assert_eq!(
        document.get::<Option<String>, _>("active_parser_version"),
        Some("legacy-v1".to_owned())
    );
    assert!(document.get::<bool, _>("ocr_required"));
    let page: (f64, f64, bool, bool) = sqlx::query_as(
        "SELECT width,height,ocr_required,active FROM document_pages WHERE document_id=$1",
    )
    .bind(document_id)
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(page, (1.0, 1.0, true, true));
    let reason_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM exclusion_reasons WHERE project_id=$1 AND stage='full_text'",
    )
    .bind(project_id)
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(reason_count, 8);
    let parse_status_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='documents' AND column_name='parse_status')",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert!(!parse_status_exists);

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&mut connection)
        .await
        .unwrap();
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM document_pages")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(remaining, 0);

    connection
        .execute("SET search_path TO public")
        .await
        .unwrap();
    let drop_schema = format!("DROP SCHEMA {schema} CASCADE");
    sqlx::query(AssertSqlSafe(drop_schema))
        .execute(&mut connection)
        .await
        .unwrap();
}
