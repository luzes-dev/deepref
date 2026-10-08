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
        Default::default(),
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

#[tokio::test]
async fn manual_values_supersede_previous_values_and_keep_history() {
    use deepref_application::ExtractionValue;
    use deepref_postgres::{
        ManualExtractionEvidence, ManualExtractionValue, clear_value, list_field_definitions,
        list_values, record_manual_value,
    };
    let Some(pool) = database().await else { return };
    let (project_id, _report_id, study_id, block_id) = seed_project(&pool).await;
    let actor = Actor::new(ActorKind::User, "reviewer-1".to_owned()).expect("actor");
    let mut ids = Vec::new();
    for (key, value_type) in [
        ("sample_size", ExtractionFieldType::Number),
        ("setting", ExtractionFieldType::Text),
    ] {
        let id = Uuid::new_v4();
        create_field_definition(
            &pool,
            ExtractionFieldDefinition {
                id,
                project_id,
                version: 1,
                field_key: key.to_owned(),
                label: key.to_owned(),
                value_type,
                required: false,
            },
        )
        .await
        .expect("field definition");
        ids.push(id);
    }
    // Fields come back in creation order, not alphabetically.
    let listed = list_field_definitions(&pool, project_id.as_uuid())
        .await
        .expect("fields");
    assert_eq!(
        listed
            .iter()
            .map(|f| f.field_key.as_str())
            .collect::<Vec<_>>(),
        ["sample_size", "setting"]
    );

    let first = record_manual_value(
        &pool,
        project_id.as_uuid(),
        study_id,
        ManualExtractionValue {
            field_id: ids[0],
            value: ExtractionValue::Number { value: 40.0 },
            rationale: Some("  ".to_owned()),
            evidence: None,
        },
        &actor,
    )
    .await
    .expect("value without evidence");
    assert!(first.source_block_id.is_none() && first.rationale.is_none());

    let document_id: Uuid =
        sqlx::query_scalar("SELECT document_id FROM document_blocks WHERE id=$1")
            .bind(block_id)
            .fetch_one(&pool)
            .await
            .expect("document");
    let second = record_manual_value(
        &pool,
        project_id.as_uuid(),
        study_id,
        ManualExtractionValue {
            field_id: ids[0],
            value: ExtractionValue::Number { value: 42.0 },
            rationale: Some("Table 1".to_owned()),
            evidence: Some(ManualExtractionEvidence {
                document_id,
                document_block_id: block_id,
            }),
        },
        &actor,
    )
    .await
    .expect("overwrite with evidence");
    assert_eq!(second.source_block_id, Some(block_id));
    let current = list_values(&pool, project_id.as_uuid(), study_id)
        .await
        .expect("values");
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].value, ExtractionValue::Number { value: 42.0 });
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM extraction_values WHERE study_id=$1")
        .bind(study_id)
        .fetch_one(&pool)
        .await
        .expect("history");
    assert_eq!(rows, 2);

    // Type mismatch, foreign evidence and unknown fields are rejected.
    assert!(matches!(
        record_manual_value(
            &pool,
            project_id.as_uuid(),
            study_id,
            ManualExtractionValue {
                field_id: ids[1],
                value: ExtractionValue::Number { value: 1.0 },
                rationale: None,
                evidence: None,
            },
            &actor,
        )
        .await,
        Err(ExtractionError::InvalidValue(_))
    ));
    assert!(matches!(
        record_manual_value(
            &pool,
            project_id.as_uuid(),
            study_id,
            ManualExtractionValue {
                field_id: ids[1],
                value: ExtractionValue::Text {
                    value: "ward".to_owned()
                },
                rationale: None,
                evidence: Some(ManualExtractionEvidence {
                    document_id,
                    document_block_id: Uuid::new_v4(),
                }),
            },
            &actor,
        )
        .await,
        Err(ExtractionError::EvidenceNotInStudy)
    ));
    assert!(matches!(
        clear_value(
            &pool,
            project_id.as_uuid(),
            study_id,
            Uuid::new_v4(),
            &actor
        )
        .await,
        Err(ExtractionError::DefinitionNotFound)
    ));

    clear_value(&pool, project_id.as_uuid(), study_id, ids[0], &actor)
        .await
        .expect("clear");
    clear_value(&pool, project_id.as_uuid(), study_id, ids[0], &actor)
        .await
        .expect("clearing twice is a no-op");
    assert!(
        list_values(&pool, project_id.as_uuid(), study_id)
            .await
            .expect("values")
            .is_empty()
    );
}

#[tokio::test]
async fn ai_act_flags_values_to_verify_skips_human_values_and_can_be_undone() {
    use deepref_application::ExtractionValue;
    use deepref_postgres::{
        ExtractionApplyOptions, ManualExtractionValue, NewActivity, confirm_value, list_values,
        record_activity_in_transaction, record_manual_value, undo_activity,
    };
    let Some(pool) = database().await else { return };
    let (project_id, report_id, study_id, block_id) = seed_project(&pool).await;
    let document_id: Uuid =
        sqlx::query_scalar("SELECT document_id FROM document_blocks WHERE id=$1")
            .bind(block_id)
            .fetch_one(&pool)
            .await
            .expect("document");
    let human = Actor::new(ActorKind::User, "reviewer-1".to_owned()).expect("actor");
    let ai = Actor::new(ActorKind::Automation, "ai:test-model".to_owned()).expect("actor");
    let mut fields = Vec::new();
    for key in ["sample_size", "setting"] {
        let id = Uuid::new_v4();
        create_field_definition(
            &pool,
            ExtractionFieldDefinition {
                id,
                project_id,
                version: 1,
                field_key: key.to_owned(),
                label: key.to_owned(),
                value_type: ExtractionFieldType::Text,
                required: false,
            },
        )
        .await
        .expect("field");
        fields.push(id);
    }
    // A person already filled the first field: the AI must not overwrite it.
    record_manual_value(
        &pool,
        project_id.as_uuid(),
        study_id,
        ManualExtractionValue {
            field_id: fields[0],
            value: ExtractionValue::Text {
                value: "human".to_owned(),
            },
            rationale: None,
            evidence: None,
        },
        &human,
    )
    .await
    .expect("manual value");
    let source = || ExtractionEvidence {
        report_id,
        document_id,
        document_block_id: block_id,
        page: 1,
        parser_version: "parser.v1".to_owned(),
        content_hash: "d".repeat(64),
    };
    let extraction = DataExtraction {
        study_id,
        fields: fields
            .iter()
            .map(|id| ExtractedField::Value {
                field_id: *id,
                field_version: 1,
                value: TypedExtractionValue::Text {
                    value: "ai".to_owned(),
                },
                rationale: "stated in the results".to_owned(),
                source: source(),
            })
            .collect(),
    };
    let run_id = Uuid::new_v4();
    let proposal_id = Uuid::new_v4();
    let hash = "a".repeat(64);
    sqlx::query(
        "INSERT INTO ai_runs
         (id,project_id,task_kind,provider,model,prompt_version,input_hash,status,profile,
          model_version,schema_version,prompt_hash,schema_hash,reuse_hash)
         VALUES ($1,$2,'data_extraction','test','test-model','v1',$3,'completed','reasoning',
                 'v1','v1',$3,$3,$3)",
    )
    .bind(run_id)
    .bind(project_id.as_uuid())
    .bind(&hash)
    .execute(&pool)
    .await
    .expect("run");
    sqlx::query(
        "INSERT INTO ai_proposals
         (id,project_id,ai_run_id,proposal_type,payload,status,entity_type,operation,
          model_run_id,authority_tier,task_kind)
         VALUES ($1,$2,$3,'data_extraction','{}'::jsonb,'pending','extraction_study',
                 'data_extraction',$3,'workflow_suggestion','data_extraction')",
    )
    .bind(proposal_id)
    .bind(project_id.as_uuid())
    .bind(run_id)
    .execute(&pool)
    .await
    .expect("proposal");
    let mut tx = pool.begin().await.expect("tx");
    let applied = apply_data_extraction_in_transaction(
        &mut tx,
        project_id,
        study_id,
        proposal_id,
        &extraction,
        &ai,
        ExtractionApplyOptions {
            needs_verification: true,
            skip_existing: true,
        },
    )
    .await
    .expect("apply");
    assert_eq!(applied.inserted_value_ids.len(), 1);
    assert_eq!(applied.skipped_existing, 1);
    let mut entry = NewActivity::new(
        project_id.as_uuid(),
        "ai",
        "test-model",
        ai.clone(),
        "extraction",
        "extraction_values_added",
        "The AI filled 1 field",
    );
    entry.after_state = serde_json::json!({"value_ids": applied.inserted_value_ids});
    entry.undo_kind = Some("extraction_values");
    let activity_id = record_activity_in_transaction(&mut tx, &entry)
        .await
        .expect("activity");
    tx.commit().await.expect("commit");

    let values = list_values(&pool, project_id.as_uuid(), study_id)
        .await
        .expect("values");
    let flagged: Vec<_> = values
        .iter()
        .filter(|value| value.needs_verification)
        .collect();
    assert_eq!(flagged.len(), 1);
    assert_eq!(values.len(), 2);

    // Confirming clears the flag, and then the AI action can no longer be undone.
    confirm_value(&pool, project_id.as_uuid(), study_id, flagged[0].id, &human)
        .await
        .expect("confirm");
    let blocked = undo_activity(&pool, project_id.as_uuid(), activity_id, &human).await;
    assert!(matches!(
        blocked,
        Err(deepref_postgres::ActivityError::CannotUndo(_))
    ));

    // A fresh unconfirmed AI value is retracted by undo.
    let second = Uuid::new_v4();
    sqlx::query(
        "UPDATE extraction_values SET needs_verification=true,verified_at=NULL
         WHERE id=$1",
    )
    .bind(flagged[0].id)
    .execute(&pool)
    .await
    .expect("reflag");
    let _ = second;
    let undone = undo_activity(&pool, project_id.as_uuid(), activity_id, &human)
        .await
        .expect("undo");
    assert!(undone.undone_at.is_some());
    let remaining = list_values(&pool, project_id.as_uuid(), study_id)
        .await
        .expect("values");
    assert_eq!(remaining.len(), 1);
    assert!(!remaining[0].needs_verification);
    assert!(matches!(
        undo_activity(&pool, project_id.as_uuid(), activity_id, &human).await,
        Err(deepref_postgres::ActivityError::AlreadyUndone)
    ));

    for table in [
        "extraction_values",
        "extraction_events",
        "ai_proposals",
        "ai_runs",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE project_id=$1"
        )))
        .bind(project_id.as_uuid())
        .execute(&pool)
        .await
        .expect("cleanup child rows");
    }
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id.as_uuid())
        .execute(&pool)
        .await
        .expect("cleanup");
}
