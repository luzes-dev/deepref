#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]
use deepref_ai::{DataExtraction, ExtractedField, ExtractionEvidence, TypedExtractionValue};
use deepref_application::{ExtractionFieldDefinition, ExtractionFieldType};
use deepref_domain::{Actor, ActorKind, ProjectId};
use deepref_postgres::{
    ExtractionError, apply_data_extraction_in_transaction, create_field_definition,
};
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

async fn seed_project(pool: &PgPool) -> (ProjectId, Uuid, Uuid, Uuid) {
    let project_id = ProjectId::new(Uuid::new_v4());
    let report_id = Uuid::new_v4();
    let study_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let block_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'extraction test')")
        .bind(project_id.as_uuid())
        .execute(pool)
        .await
        .expect("project inserts");
    sqlx::query(
        "INSERT INTO reports (id,title) VALUES ($1,'Extraction report')
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(report_id)
    .execute(pool)
    .await
    .expect("report inserts");
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project_id.as_uuid())
        .bind(report_id)
        .execute(pool)
        .await
        .expect("project report inserts");
    sqlx::query(
        "INSERT INTO studies
         (id,project_id,title,design_context,study_revision,updated_by_actor_kind,updated_by_actor_id)
         VALUES ($1,$2,'Extraction study','{}'::jsonb,0,'system','extraction-test')",
    )
    .bind(study_id)
    .bind(project_id.as_uuid())
    .execute(pool)
    .await
    .expect("study inserts");
    sqlx::query(
        "INSERT INTO study_reports (project_id,study_id,report_id,relationship)
         VALUES ($1,$2,$3,'report_of_study')",
    )
    .bind(project_id.as_uuid())
    .bind(study_id)
    .bind(report_id)
    .execute(pool)
    .await
    .expect("study membership inserts");
    sqlx::query(
        "INSERT INTO documents
         (id,project_id,report_id,object_key,content_hash,mime_type,byte_size,source,status,
          actor_kind,actor_id,active_parser_version,parser_version)
         VALUES ($1,$2,$3,$4,$5,'application/pdf',1,'upload','available',
                 'system','extraction-test','parser.v1','parser.v1')",
    )
    .bind(document_id)
    .bind(project_id.as_uuid())
    .bind(report_id)
    .bind(format!("documents/{document_id}"))
    .bind("c".repeat(64))
    .execute(pool)
    .await
    .expect("document inserts");
    sqlx::query(
        "INSERT INTO document_pages(document_id,parser_version,page_number,width,height,active)
         VALUES ($1,'parser.v1',1,100,100,true)",
    )
    .bind(document_id)
    .execute(pool)
    .await
    .expect("page inserts");
    sqlx::query(
        "INSERT INTO document_blocks
         (id,document_id,parser_version,page_number,kind,section_path,ordinal,text,content_hash,active)
         VALUES ($1,$2,'parser.v1',1,'text',ARRAY['Results'],0,'A value was reported',$3,true)",
    )
    .bind(block_id)
    .bind(document_id)
    .bind("d".repeat(64))
    .execute(pool)
    .await
    .expect("block inserts");
    (project_id, report_id, study_id, block_id)
}

#[tokio::test]
async fn list_values_distinguishes_existing_empty_study_from_missing_or_cross_project_study() {
    let Some(pool) = database().await else { return };
    let (project_id, _report_id, study_id, _block_id) = seed_project(&pool).await;

    let empty = deepref_postgres::list_values(&pool, project_id.as_uuid(), study_id)
        .await
        .expect("existing study with no values is an empty extraction");
    assert!(empty.is_empty());

    let other_project = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'other extraction project')")
        .bind(other_project)
        .execute(&pool)
        .await
        .expect("other project inserts");
    assert!(matches!(
        deepref_postgres::list_values(&pool, other_project, study_id).await,
        Err(ExtractionError::StudyNotFound)
    ));
    assert!(matches!(
        deepref_postgres::list_values(&pool, project_id.as_uuid(), Uuid::new_v4()).await,
        Err(ExtractionError::StudyNotFound)
    ));

    sqlx::query("DELETE FROM projects WHERE id IN ($1,$2)")
        .bind(project_id.as_uuid())
        .bind(other_project)
        .execute(&pool)
        .await
        .expect("extraction list test cleanup");
}

#[tokio::test]
async fn extraction_acceptance_rejects_a_pending_value_from_an_old_field_version() {
    let Some(pool) = database().await else { return };
    let (project_id, report_id, study_id, block_id) = seed_project(&pool).await;
    let field_id = Uuid::new_v4();
    create_field_definition(
        &pool,
        ExtractionFieldDefinition {
            id: field_id,
            project_id,
            version: 1,
            field_key: "sample_size".to_owned(),
            label: "Sample size".to_owned(),
            value_type: ExtractionFieldType::Text,
            required: false,
        },
    )
    .await
    .expect("v1 field definition");
    let latest = create_field_definition(
        &pool,
        ExtractionFieldDefinition {
            id: field_id,
            project_id,
            version: 2,
            field_key: "sample_size".to_owned(),
            label: "Sample size (updated)".to_owned(),
            value_type: ExtractionFieldType::Text,
            required: false,
        },
    )
    .await
    .expect("v2 field definition");
    assert_eq!(
        deepref_postgres::list_field_definitions(&pool, project_id.as_uuid())
            .await
            .expect("current field definitions")[0]
            .version,
        latest.version
    );

    let extraction = DataExtraction {
        study_id,
        fields: vec![ExtractedField::Value {
            field_id,
            field_version: 1,
            value: TypedExtractionValue::Text {
                value: "42".to_owned(),
            },
            rationale: "The report states the sample size.".to_owned(),
            source: ExtractionEvidence {
                report_id,
                document_id: sqlx::query_scalar(
                    "SELECT document_id FROM document_blocks WHERE id=$1",
                )
                .bind(block_id)
                .fetch_one(&pool)
                .await
                .expect("source document"),
                document_block_id: block_id,
                page: 1,
                parser_version: "parser.v1".to_owned(),
                content_hash: "d".repeat(64),
            },
        }],
    };
    let actor = Actor::new(ActorKind::User, "extraction-reviewer").expect("actor");
    let mut tx = pool.begin().await.expect("acceptance transaction");
    let error = apply_data_extraction_in_transaction(
        &mut tx,
        project_id,
        study_id,
        Uuid::new_v4(),
        &extraction,
        &actor,
    )
    .await
    .expect_err("stale extraction proposal must be rejected");
    assert!(matches!(error, ExtractionError::StaleDefinitionVersion));
    tx.rollback().await.expect("rollback stale acceptance");

    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id.as_uuid())
        .execute(&pool)
        .await
        .expect("extraction version test cleanup");
}
