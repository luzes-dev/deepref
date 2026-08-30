use anyhow::{Result, ensure};
use deepref_application::{
    AutomationDefinitionStatus, AutomationDomainEvent, AutomationTriggerKind,
    BuiltInAutomationRecipe, ConfigureAutomationDefinition,
};
use deepref_domain::{Actor, ActorKind, ProjectId};
use deepref_postgres::{
    configure_automation_definition, dispatch_automation_domain_event, migrate,
};
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

fn actor(id: &str) -> Actor {
    Actor::new(ActorKind::User, id).expect("valid test actor")
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

async fn configure(
    pool: &PgPool,
    project_id: ProjectId,
    trigger: AutomationTriggerKind,
    status: AutomationDefinitionStatus,
) -> Result<()> {
    configure_automation_definition(
        pool,
        &ConfigureAutomationDefinition::new(
            project_id,
            format!("{} {status:?}", trigger.as_str()),
            trigger,
            BuiltInAutomationRecipe::ProjectMaintenanceV1,
            status,
            actor("automation-event-test-user"),
        )?,
    )
    .await?;
    Ok(())
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

fn events(project_id: ProjectId) -> Vec<AutomationDomainEvent> {
    vec![
        AutomationDomainEvent::ReportAdded {
            project_id,
            report_id: Uuid::new_v4(),
        },
        AutomationDomainEvent::AcquisitionCompleted {
            project_id,
            acquisition_id: Uuid::new_v4(),
        },
        AutomationDomainEvent::FullTextAttached {
            project_id,
            document_id: Uuid::new_v4(),
            actor: actor("document-source-user"),
        },
        AutomationDomainEvent::ReportIncluded {
            project_id,
            screening_event_id: Uuid::new_v4(),
            actor: actor("screening-source-user"),
        },
        AutomationDomainEvent::StudyCreated {
            project_id,
            study_event_id: Uuid::new_v4(),
            actor: actor("study-source-user"),
        },
        AutomationDomainEvent::AppraisalCompleted {
            project_id,
            appraisal_event_id: Uuid::new_v4(),
            actor: actor("appraisal-source-user"),
        },
    ]
}

#[tokio::test]
async fn all_typed_domain_events_dispatch_once_to_active_definitions() -> Result<()> {
    let _guard = test_guard().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let project_id = project(&pool, "typed automation event mapping").await;
    let result = async {
        for trigger in AutomationTriggerKind::ALL
            .into_iter()
            .filter(|trigger| *trigger != AutomationTriggerKind::Manual)
        {
            configure(
                &pool,
                project_id,
                trigger,
                AutomationDefinitionStatus::Active,
            )
            .await?;
            configure(
                &pool,
                project_id,
                trigger,
                AutomationDefinitionStatus::Paused,
            )
            .await?;
        }

        let events = events(project_id);
        let mut transaction = pool.begin().await?;
        for event in &events {
            ensure!(dispatch_automation_domain_event(&mut transaction, event).await? == 1);
            ensure!(dispatch_automation_domain_event(&mut transaction, event).await? == 0);
        }
        transaction.commit().await?;

        let rows = sqlx::query(
            "SELECT trigger_kind,trigger_reference,idempotency_key,actor_kind,actor_id
             FROM automation_runs WHERE project_id=$1 ORDER BY trigger_kind",
        )
        .bind(project_id.as_uuid())
        .fetch_all(&pool)
        .await?;
        ensure!(rows.len() == events.len());
        for event in &events {
            let identity = event.source_identity();
            let row = rows
                .iter()
                .find(|row| row.get::<String, _>("trigger_kind") == event.trigger().as_str())
                .expect("every typed event creates its matching run");
            ensure!(row.get::<String, _>("trigger_reference") == identity);
            ensure!(row.get::<String, _>("idempotency_key") == identity);
        }
        ensure!(
            rows.iter()
                .filter(|row| row.get::<String, _>("actor_kind") == "system")
                .all(|row| row.get::<String, _>("actor_id") == "automation-domain-event")
        );
        let project_jobs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE project_id=$1 AND kind='automation_run'",
        )
        .bind(project_id.as_uuid())
        .fetch_one(&pool)
        .await?;
        ensure!(project_jobs == events.len() as i64);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &[project_id]).await?;
    result
}

#[tokio::test]
async fn dispatch_rolls_back_with_source_transaction_and_is_project_isolated() -> Result<()> {
    let _guard = test_guard().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let source_project = project(&pool, "automation rollback source").await;
    let other_project = project(&pool, "automation rollback isolated").await;
    let result = async {
        configure(
            &pool,
            source_project,
            AutomationTriggerKind::ReportAdded,
            AutomationDefinitionStatus::Active,
        )
        .await?;
        configure(
            &pool,
            other_project,
            AutomationTriggerKind::ReportAdded,
            AutomationDefinitionStatus::Active,
        )
        .await?;
        let report_id = Uuid::new_v4();
        sqlx::query("INSERT INTO reports (id,title) VALUES ($1,'rollback report')")
            .bind(report_id)
            .execute(&pool)
            .await?;
        let mut transaction = pool.begin().await?;
        sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
            .bind(source_project.as_uuid())
            .bind(report_id)
            .execute(&mut *transaction)
            .await?;
        ensure!(
            dispatch_automation_domain_event(
                &mut transaction,
                &AutomationDomainEvent::ReportAdded {
                    project_id: source_project,
                    report_id,
                },
            )
            .await?
                == 1
        );
        transaction.rollback().await?;
        let source_runs: i64 =
            sqlx::query_scalar("SELECT count(*) FROM automation_runs WHERE project_id=$1")
                .bind(source_project.as_uuid())
                .fetch_one(&pool)
                .await?;
        let other_runs: i64 =
            sqlx::query_scalar("SELECT count(*) FROM automation_runs WHERE project_id=$1")
                .bind(other_project.as_uuid())
                .fetch_one(&pool)
                .await?;
        ensure!(source_runs == 0 && other_runs == 0);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &[source_project, other_project]).await?;
    result
}

#[tokio::test]
async fn scientific_tables_have_no_automation_transition_triggers() -> Result<()> {
    let _guard = test_guard().await;
    let Some(pool) = database().await else {
        return Ok(());
    };
    let trigger_count: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM information_schema.triggers
         WHERE event_object_table = ANY($1)
           AND trigger_name LIKE 'automation_%'",
    )
    .bind(vec![
        "project_reports",
        "acquisition_runs",
        "documents",
        "screening_events",
        "study_events",
        "appraisal_events",
    ])
    .fetch_one(&pool)
    .await?;
    ensure!(trigger_count == 0);
    Ok(())
}
