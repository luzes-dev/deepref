use anyhow::{Result, ensure};
use deepref_application::{
    AutomationDefinitionStatus, AutomationTriggerKind, BuiltInAutomationRecipe,
    ConfigureAutomationDefinition,
};
use deepref_domain::{Actor, ActorKind, ProjectId};
use deepref_postgres::{configure_automation_definition, migrate};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use std::sync::OnceLock;
use tokio::sync::Mutex;
use uuid::Uuid;

static DATABASE_TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(12)
        .connect(&url)
        .await
        .expect("DATABASE_URL database must be reachable");
    migrate(&pool)
        .await
        .expect("DATABASE_URL migrations must apply");
    Some(pool)
}

async fn test_guard() -> tokio::sync::MutexGuard<'static, ()> {
    DATABASE_TEST_MUTEX
        .get_or_init(|| Mutex::const_new(()))
        .lock()
        .await
}

fn actor() -> Actor {
    Actor::new(ActorKind::User, "automation-event-test-user").expect("valid test actor")
}

async fn project(pool: &PgPool, name: &str) -> ProjectId {
    let project_id = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,$2)")
        .bind(project_id.as_uuid())
        .bind(name)
        .execute(pool)
        .await
        .expect("project fixture inserts");
    project_id
}

async fn report(pool: &PgPool, title: &str) -> Uuid {
    let report_id = Uuid::new_v4();
    sqlx::query("INSERT INTO reports (id,title) VALUES ($1,$2)")
        .bind(report_id)
        .bind(title)
        .execute(pool)
        .await
        .expect("report fixture inserts");
    report_id
}

async fn configure(
    pool: &PgPool,
    project_id: ProjectId,
    name: &str,
    trigger: AutomationTriggerKind,
    status: AutomationDefinitionStatus,
) -> Result<Uuid> {
    let definition = configure_automation_definition(
        pool,
        &ConfigureAutomationDefinition::new(
            project_id,
            name,
            trigger,
            BuiltInAutomationRecipe::ProjectMaintenanceV1,
            status,
            actor(),
        )?,
    )
    .await?;
    Ok(definition.id.as_uuid())
}

async fn run_count(pool: &PgPool, project_id: ProjectId, trigger: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM automation_runs WHERE project_id=$1 AND trigger_kind=$2",
    )
    .bind(project_id.as_uuid())
    .bind(trigger)
    .fetch_one(pool)
    .await
    .expect("automation run count query")
}

async fn run_for_identity(
    pool: &PgPool,
    project_id: ProjectId,
    trigger: &str,
    identity: &str,
) -> Result<sqlx::postgres::PgRow> {
    Ok(sqlx::query(
        "SELECT trigger_reference,idempotency_key,actor_kind,actor_id
         FROM automation_runs
         WHERE project_id=$1 AND trigger_kind=$2 AND trigger_reference=$3",
    )
    .bind(project_id.as_uuid())
    .bind(trigger)
    .bind(identity)
    .fetch_one(pool)
    .await?)
}

async fn cleanup(pool: &PgPool, project_ids: &[ProjectId]) -> Result<()> {
    let ids = project_ids
        .iter()
        .map(|project_id| project_id.as_uuid())
        .collect::<Vec<_>>();
    sqlx::query("DELETE FROM projects WHERE id = ANY($1)")
        .bind(ids)
        .execute(pool)
        .await?;
    Ok(())
}

async fn configure_all_triggers(pool: &PgPool, project_id: ProjectId) -> Result<()> {
    for (name, trigger) in [
        ("active report added", AutomationTriggerKind::ReportAdded),
        (
            "active acquisition completed",
            AutomationTriggerKind::AcquisitionCompleted,
        ),
        (
            "active full text attached",
            AutomationTriggerKind::FullTextAttached,
        ),
        (
            "active report included",
            AutomationTriggerKind::ReportIncluded,
        ),
        ("active study created", AutomationTriggerKind::StudyCreated),
        (
            "active appraisal completed",
            AutomationTriggerKind::AppraisalCompleted,
        ),
    ] {
        configure(
            pool,
            project_id,
            name,
            trigger,
            AutomationDefinitionStatus::Active,
        )
        .await?;
        configure(
            pool,
            project_id,
            &format!("paused {name}"),
            trigger,
            AutomationDefinitionStatus::Paused,
        )
        .await?;
    }
    Ok(())
}

async fn insert_completed_acquisition(
    pool: &PgPool,
    project_id: ProjectId,
    run_id: Uuid,
    status: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO acquisition_runs
         (id,project_id,legacy_ingestion_id,source,strategy,format,idempotency_key,config,metadata,
          status,max_depth,seed_count,queued_count,fetched_count,failed_count,
          metadata_provider,citation_provider,created_at,completed_at)
         VALUES ($1,$2,NULL,'event-test','event_test',NULL,$3,'{}'::jsonb,'{}'::jsonb,$4,
                 0,0,0,0,0,'','',now(),CASE WHEN $4='completed' THEN now() ELSE NULL END)",
    )
    .bind(run_id)
    .bind(project_id.as_uuid())
    .bind(format!("event-test-{run_id}"))
    .bind(status)
    .execute(pool)
    .await?;
    Ok(())
}

async fn insert_uploaded_document(
    pool: &PgPool,
    project_id: ProjectId,
    report_id: Uuid,
    document_id: Uuid,
    actor_kind: &str,
    actor_id: &str,
) -> Result<()> {
    let object_key = format!("documents/{document_id}");
    let content_hash = format!("{:064x}", document_id.as_u128());
    sqlx::query(
        "INSERT INTO documents
         (id,project_id,report_id,object_key,content_hash,mime_type,byte_size,original_filename,
          source,status,external_url,actor_kind,actor_id,content_available_at)
         VALUES ($1,$2,$3,$4,$5,'application/pdf',123,'event-test.pdf','upload','uploaded',NULL,
                 $6,$7,now())",
    )
    .bind(document_id)
    .bind(project_id.as_uuid())
    .bind(report_id)
    .bind(object_key)
    .bind(content_hash)
    .bind(actor_kind)
    .bind(actor_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn insert_report_inclusion_event(
    pool: &PgPool,
    project_id: ProjectId,
    report_id: Uuid,
    event_id: Uuid,
    actor_kind: &str,
    actor_id: &str,
) -> Result<()> {
    let protocol_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO protocol_versions
         (id,project_id,version,name,status,criteria,objective,question)
         VALUES ($1,$2,1,'Event test protocol','published','[]'::jsonb,'','')",
    )
    .bind(protocol_id)
    .bind(project_id.as_uuid())
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO screening_state
         (project_id,report_id,title_abstract_status,full_text_status,final_status,revision)
         VALUES ($1,$2,'include','unscreened','pending_full_text',0)",
    )
    .bind(project_id.as_uuid())
    .bind(report_id)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO screening_events
         (id,project_id,report_id,event_kind,stage,decision,protocol_version_id,actor_kind,actor_id,
          previous_title_abstract_status,previous_full_text_status,previous_final_status,
          result_title_abstract_status,result_full_text_status,result_final_status)
         VALUES ($1,$2,$3,'decision','full_text','include',$4,$5,$6,
                 'include','unscreened','pending_full_text','include','include','include')",
    )
    .bind(event_id)
    .bind(project_id.as_uuid())
    .bind(report_id)
    .bind(protocol_id)
    .bind(actor_kind)
    .bind(actor_id)
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE screening_state
         SET full_text_status='include',final_status='include',revision=1,last_event_id=$3
         WHERE project_id=$1 AND report_id=$2",
    )
    .bind(project_id.as_uuid())
    .bind(report_id)
    .bind(event_id)
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE project_reports SET lifecycle_status='included'
         WHERE project_id=$1 AND report_id=$2",
    )
    .bind(project_id.as_uuid())
    .bind(report_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn insert_repeated_report_inclusion_event(
    pool: &PgPool,
    project_id: ProjectId,
    report_id: Uuid,
    event_id: Uuid,
    actor_kind: &str,
    actor_id: &str,
    undoes_event_id: Option<Uuid>,
) -> Result<()> {
    let protocol_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM protocol_versions
         WHERE project_id=$1
         ORDER BY version DESC, id DESC
         LIMIT 1",
    )
    .bind(project_id.as_uuid())
    .fetch_one(pool)
    .await?;
    sqlx::query(
        "INSERT INTO screening_events
         (id,project_id,report_id,event_kind,stage,decision,undoes_event_id,
          protocol_version_id,actor_kind,actor_id,
          previous_title_abstract_status,previous_full_text_status,previous_final_status,
          result_title_abstract_status,result_full_text_status,result_final_status)
         VALUES ($1,$2,$3,$4,'full_text',$5,$6,$7,$8,$9,
                 'include','unscreened',$10,'include','include','include')",
    )
    .bind(event_id)
    .bind(project_id.as_uuid())
    .bind(report_id)
    .bind(if undoes_event_id.is_some() {
        "undo"
    } else {
        "decision"
    })
    .bind(if undoes_event_id.is_some() {
        None::<&str>
    } else {
        Some("include")
    })
    .bind(undoes_event_id)
    .bind(protocol_id)
    .bind(actor_kind)
    .bind(actor_id)
    .bind(if undoes_event_id.is_some() {
        "pending_full_text"
    } else {
        "include"
    })
    .execute(pool)
    .await?;
    Ok(())
}

async fn insert_study_created_event(
    pool: &PgPool,
    project_id: ProjectId,
    study_id: Uuid,
    event_id: Uuid,
    actor_kind: &str,
    actor_id: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO studies
         (id,project_id,title,design,design_context,study_revision,updated_by_actor_kind,
          updated_by_actor_id)
         VALUES ($1,$2,'Event test study',NULL,'{}'::jsonb,0,$3,$4)",
    )
    .bind(study_id)
    .bind(project_id.as_uuid())
    .bind(actor_kind)
    .bind(actor_id)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO study_events
         (id,project_id,study_id,event_type,result_study_id,before_revision,result_revision,
          before_snapshot,result_snapshot,payload,actor_kind,actor_id)
         VALUES ($1,$2,$3,'study_created',$3,0,0,'{}'::jsonb,
                 '{\"title\":\"Event test study\"}'::jsonb,'{}'::jsonb,$4,$5)",
    )
    .bind(event_id)
    .bind(project_id.as_uuid())
    .bind(study_id)
    .bind(actor_kind)
    .bind(actor_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn insert_appraisal_completed_event(
    pool: &PgPool,
    project_id: ProjectId,
    report_id: Uuid,
    assessment_id: Uuid,
    event_id: Uuid,
    actor_kind: &str,
    actor_id: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO appraisal_assessments
         (id,project_id,report_id,definition_id,definition_version,responses,judgments,
          actor_kind,actor_id)
         VALUES ($1,$2,$3,'event-test',1,'{}'::jsonb,'{}'::jsonb,$4,$5)",
    )
    .bind(assessment_id)
    .bind(project_id.as_uuid())
    .bind(report_id)
    .bind(actor_kind)
    .bind(actor_id)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO appraisal_events
         (id,assessment_id,project_id,report_id,event_type,payload,actor_kind,actor_id)
         VALUES ($1,$2,$3,$4,'appraisal_completed','{}'::jsonb,$5,$6)",
    )
    .bind(event_id)
    .bind(assessment_id)
    .bind(project_id.as_uuid())
    .bind(report_id)
    .bind(actor_kind)
    .bind(actor_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[tokio::test]
async fn all_six_authoritative_events_dispatch_active_definitions() -> Result<()> {
    let _guard = test_guard().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool, "automation event mapping fixture").await;
    let report_id = report(&pool, "Automation event mapping report").await;
    let result = async {
        configure_all_triggers(&pool, project_id).await?;

        sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
            .bind(project_id.as_uuid())
            .bind(report_id)
            .execute(&pool)
            .await?;

        let completed_on_insert = Uuid::new_v4();
        insert_completed_acquisition(&pool, project_id, completed_on_insert, "completed").await?;
        let completed_on_transition = Uuid::new_v4();
        insert_completed_acquisition(&pool, project_id, completed_on_transition, "queued").await?;
        sqlx::query(
            "UPDATE acquisition_runs SET status='completed',completed_at=now()
             WHERE project_id=$1 AND id=$2",
        )
        .bind(project_id.as_uuid())
        .bind(completed_on_transition)
        .execute(&pool)
        .await?;
        sqlx::query(
            "UPDATE acquisition_runs SET status='completed',completed_at=now()
             WHERE project_id=$1 AND id=$2",
        )
        .bind(project_id.as_uuid())
        .bind(completed_on_transition)
        .execute(&pool)
        .await?;

        let document_id = Uuid::new_v4();
        insert_uploaded_document(
            &pool,
            project_id,
            report_id,
            document_id,
            "user",
            "document-source-user",
        )
        .await?;

        let inclusion_event_id = Uuid::new_v4();
        insert_report_inclusion_event(
            &pool,
            project_id,
            report_id,
            inclusion_event_id,
            "user",
            "screening-source-user",
        )
        .await?;

        let study_event_id = Uuid::new_v4();
        insert_study_created_event(
            &pool,
            project_id,
            Uuid::new_v4(),
            study_event_id,
            "user",
            "study-source-user",
        )
        .await?;

        let appraisal_event_id = Uuid::new_v4();
        insert_appraisal_completed_event(
            &pool,
            project_id,
            report_id,
            Uuid::new_v4(),
            appraisal_event_id,
            "user",
            "appraisal-source-user",
        )
        .await?;

        ensure!(run_count(&pool, project_id, "report_added").await == 1);
        ensure!(run_count(&pool, project_id, "acquisition_completed").await == 2);
        ensure!(run_count(&pool, project_id, "full_text_attached").await == 1);
        ensure!(run_count(&pool, project_id, "report_included").await == 1);
        ensure!(run_count(&pool, project_id, "study_created").await == 1);
        ensure!(run_count(&pool, project_id, "appraisal_completed").await == 1);
        ensure!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM automation_runs WHERE project_id=$1"
            )
            .bind(project_id.as_uuid())
            .fetch_one(&pool)
            .await?
                == 7
        );

        for (trigger, identity, source_actor) in [
            (
                "report_added",
                format!("project_report:{}:{report_id}", project_id.as_uuid()),
                ("system", "automation-event-bridge"),
            ),
            (
                "acquisition_completed",
                format!("acquisition_run:{completed_on_insert}"),
                ("system", "automation-event-bridge"),
            ),
            (
                "full_text_attached",
                format!("document:{document_id}"),
                ("user", "document-source-user"),
            ),
            (
                "report_included",
                format!("screening_event:{inclusion_event_id}"),
                ("user", "screening-source-user"),
            ),
            (
                "study_created",
                format!("study_event:{study_event_id}"),
                ("user", "study-source-user"),
            ),
            (
                "appraisal_completed",
                format!("appraisal_event:{appraisal_event_id}"),
                ("user", "appraisal-source-user"),
            ),
        ] {
            let row = run_for_identity(&pool, project_id, trigger, &identity).await?;
            ensure!(row.get::<String, _>("trigger_reference") == identity);
            ensure!(row.get::<String, _>("idempotency_key") == identity);
            ensure!(row.get::<String, _>("actor_kind") == source_actor.0);
            ensure!(row.get::<String, _>("actor_id") == source_actor.1);
        }

        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &[project_id]).await?;
    result
}

#[tokio::test]
async fn external_retrieval_fires_on_first_attachment_only_and_replay_is_idempotent() -> Result<()>
{
    let _guard = test_guard().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool, "automation external document fixture").await;
    let report_id = report(&pool, "Automation external document report").await;
    let result = async {
        configure(
            &pool,
            project_id,
            "external full text",
            AutomationTriggerKind::FullTextAttached,
            AutomationDefinitionStatus::Active,
        )
        .await?;
        let document_id = Uuid::new_v4();
        sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
            .bind(project_id.as_uuid())
            .bind(report_id)
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO documents
             (id,project_id,report_id,object_key,content_hash,mime_type,byte_size,
              original_filename,source,status,external_url,actor_kind,actor_id)
             VALUES ($1,$2,$3,NULL,NULL,'application/pdf',0,NULL,'external_url','external',
                     'https://example.test/article.pdf','system','retrieval-source')",
        )
        .bind(document_id)
        .bind(project_id.as_uuid())
        .bind(report_id)
        .execute(&pool)
        .await?;
        ensure!(run_count(&pool, project_id, "full_text_attached").await == 0);

        sqlx::query("UPDATE documents SET status='retrieving' WHERE id=$1")
            .bind(document_id)
            .execute(&pool)
            .await?;
        ensure!(run_count(&pool, project_id, "full_text_attached").await == 0);

        let object_key = format!("documents/{document_id}");
        let content_hash = format!("{:064x}", document_id.as_u128());
        sqlx::query(
            "UPDATE documents
             SET status='uploaded',object_key=$2,content_hash=$3,byte_size=321,
                 content_available_at=now(),updated_at=now()
             WHERE id=$1",
        )
        .bind(document_id)
        .bind(object_key)
        .bind(content_hash)
        .execute(&pool)
        .await?;
        ensure!(run_count(&pool, project_id, "full_text_attached").await == 1);

        sqlx::query(
            "UPDATE documents
             SET status='available',parser_version='event-parser',active_parser_version='event-parser'
             WHERE id=$1",
        )
            .bind(document_id)
            .execute(&pool)
        .await?;
        ensure!(run_count(&pool, project_id, "full_text_attached").await == 1);

        // There is no separate `parsing` status in the document schema; a
        // second retrieving/available cycle exercises the same parser
        // lifecycle while keeping the document id stable.
        sqlx::query(
            "UPDATE documents
             SET status='retrieving',object_key=NULL,content_hash=NULL,byte_size=0,
                 content_available_at=NULL,active_parser_version=NULL,parsed_at=NULL,
                 failed_at=NULL
             WHERE id=$1",
        )
        .bind(document_id)
        .execute(&pool)
        .await?;
        ensure!(run_count(&pool, project_id, "full_text_attached").await == 1);

        sqlx::query(
            "UPDATE documents
             SET status='available',object_key=$2,content_hash=$3,byte_size=321,
                 content_available_at=now(),active_parser_version='event-parser'
             WHERE id=$1",
        )
        .bind(document_id)
        .bind(format!("documents/{document_id}"))
        .bind(format!("{:064x}", document_id.as_u128()))
        .execute(&pool)
        .await?;
        ensure!(run_count(&pool, project_id, "full_text_attached").await == 1);

        let identity = format!("document:{document_id}");
        sqlx::query(
            "SELECT dispatch_automation_domain_event($1,'full_text_attached',$2,'system','replay')",
        )
        .bind(project_id.as_uuid())
        .bind(&identity)
        .fetch_one(&pool)
        .await?;
        ensure!(run_count(&pool, project_id, "full_text_attached").await == 1);
        let row = run_for_identity(&pool, project_id, "full_text_attached", &identity).await?;
        ensure!(row.get::<String, _>("actor_id") == "retrieval-source");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &[project_id]).await?;
    result
}

#[tokio::test]
async fn oversized_source_actor_uses_bridge_identity_without_rolling_back_source_event()
-> Result<()> {
    let _guard = test_guard().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool, "automation oversized actor fixture").await;
    let report_id = report(&pool, "Automation oversized actor report").await;
    let document_id = Uuid::new_v4();
    let oversized_actor_id = "a".repeat(201);
    let result = async {
        configure(
            &pool,
            project_id,
            "oversized actor full text",
            AutomationTriggerKind::FullTextAttached,
            AutomationDefinitionStatus::Active,
        )
        .await?;
        sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
            .bind(project_id.as_uuid())
            .bind(report_id)
            .execute(&pool)
            .await?;

        insert_uploaded_document(
            &pool,
            project_id,
            report_id,
            document_id,
            "user",
            &oversized_actor_id,
        )
        .await?;

        ensure!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM documents WHERE id=$1 AND actor_id=$2",
            )
            .bind(document_id)
            .bind(&oversized_actor_id)
            .fetch_one(&pool)
            .await?
                == 1
        );
        ensure!(run_count(&pool, project_id, "full_text_attached").await == 1);
        let identity = format!("document:{document_id}");
        let row = run_for_identity(&pool, project_id, "full_text_attached", &identity).await?;
        ensure!(row.get::<String, _>("actor_kind") == "system");
        ensure!(row.get::<String, _>("actor_id") == "automation-event-bridge");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &[project_id]).await?;
    result
}

#[tokio::test]
async fn report_inclusion_dispatch_requires_transition_and_ignores_undo_events() -> Result<()> {
    let _guard = test_guard().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool, "automation inclusion transition fixture").await;
    let report_id = report(&pool, "Automation inclusion transition report").await;
    let inclusion_event_id = Uuid::new_v4();
    let result = async {
        configure(
            &pool,
            project_id,
            "transition report inclusion",
            AutomationTriggerKind::ReportIncluded,
            AutomationDefinitionStatus::Active,
        )
        .await?;
        sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
            .bind(project_id.as_uuid())
            .bind(report_id)
            .execute(&pool)
            .await?;

        insert_report_inclusion_event(
            &pool,
            project_id,
            report_id,
            inclusion_event_id,
            "user",
            "transition-source-user",
        )
        .await?;
        ensure!(run_count(&pool, project_id, "report_included").await == 1);

        insert_repeated_report_inclusion_event(
            &pool,
            project_id,
            report_id,
            Uuid::new_v4(),
            "user",
            "repeated-source-user",
            None,
        )
        .await?;
        ensure!(run_count(&pool, project_id, "report_included").await == 1);

        insert_repeated_report_inclusion_event(
            &pool,
            project_id,
            report_id,
            Uuid::new_v4(),
            "user",
            "undo-source-user",
            Some(inclusion_event_id),
        )
        .await?;
        ensure!(run_count(&pool, project_id, "report_included").await == 1);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &[project_id]).await?;
    result
}

#[tokio::test]
async fn paused_no_match_and_cross_project_events_are_no_ops() -> Result<()> {
    let _guard = test_guard().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let paused_project = project(&pool, "automation paused fixture").await;
    let no_match_project = project(&pool, "automation no match fixture").await;
    let isolated_project = project(&pool, "automation isolated definition fixture").await;
    let source_project = project(&pool, "automation isolated source fixture").await;
    let result = async {
        configure(
            &pool,
            paused_project,
            "paused report added",
            AutomationTriggerKind::ReportAdded,
            AutomationDefinitionStatus::Paused,
        )
        .await?;
        configure(
            &pool,
            isolated_project,
            "isolated report added",
            AutomationTriggerKind::ReportAdded,
            AutomationDefinitionStatus::Active,
        )
        .await?;
        let paused_report = report(&pool, "Paused report").await;
        let no_match_report = report(&pool, "No match report").await;
        let isolated_report = report(&pool, "Isolated report").await;

        sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
            .bind(paused_project.as_uuid())
            .bind(paused_report)
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
            .bind(no_match_project.as_uuid())
            .bind(no_match_report)
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
            .bind(source_project.as_uuid())
            .bind(isolated_report)
            .execute(&pool)
            .await?;

        ensure!(run_count(&pool, paused_project, "report_added").await == 0);
        ensure!(run_count(&pool, no_match_project, "report_added").await == 0);
        ensure!(run_count(&pool, isolated_project, "report_added").await == 0);
        ensure!(run_count(&pool, source_project, "report_added").await == 0);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(
        &pool,
        &[
            paused_project,
            no_match_project,
            isolated_project,
            source_project,
        ],
    )
    .await?;
    result
}

#[tokio::test]
async fn source_event_and_automation_dispatch_rollback_together_and_duplicate_membership_is_safe()
-> Result<()> {
    let _guard = test_guard().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool, "automation transaction fixture").await;
    let report_id = report(&pool, "Automation transaction report").await;
    let result = async {
        configure(
            &pool,
            project_id,
            "transaction report added",
            AutomationTriggerKind::ReportAdded,
            AutomationDefinitionStatus::Active,
        )
        .await?;

        let mut transaction = pool.begin().await?;
        sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
            .bind(project_id.as_uuid())
            .bind(report_id)
            .execute(&mut *transaction)
            .await?;
        ensure!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM automation_runs WHERE project_id=$1"
            )
            .bind(project_id.as_uuid())
            .fetch_one(&mut *transaction)
            .await?
                == 1
        );
        transaction.rollback().await?;
        ensure!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM project_reports WHERE project_id=$1 AND report_id=$2"
            )
            .bind(project_id.as_uuid())
            .bind(report_id)
            .fetch_one(&pool)
            .await?
                == 0
        );
        ensure!(run_count(&pool, project_id, "report_added").await == 0);

        sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
            .bind(project_id.as_uuid())
            .bind(report_id)
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)
             ON CONFLICT (project_id,report_id) DO NOTHING",
        )
        .bind(project_id.as_uuid())
        .bind(report_id)
        .execute(&pool)
        .await?;
        ensure!(run_count(&pool, project_id, "report_added").await == 1);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &[project_id]).await?;
    result
}
