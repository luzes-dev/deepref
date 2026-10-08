#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
use deepref_application::workflows::{AutonomyLevel, AutonomyTask};
use deepref_domain::{Actor, ActorKind};
use deepref_postgres::{
    NewReviewerDecision, ResolveConflict, get_autonomy_settings,
    insert_reviewer_decision_in_transaction, list_reviewer_decisions, resolve_autonomy_level,
    resolve_reviewer_conflict, set_autonomy_level,
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .ok()?;
    deepref_postgres::migrate(&pool).await.ok()?;
    Some(pool)
}

async fn project(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'autonomy test')")
        .bind(id)
        .execute(pool)
        .await
        .expect("project");
    id
}

#[tokio::test]
async fn defaults_match_the_product_decision_and_locked_work_is_not_storable() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool).await;
    let settings = get_autonomy_settings(&pool, project_id)
        .await
        .expect("settings");
    let level = |task| {
        settings
            .iter()
            .find(|s| s.task == task)
            .expect("task")
            .level
    };
    assert_eq!(level(AutonomyTask::ExactDuplicates), AutonomyLevel::Act);
    assert_eq!(level(AutonomyTask::FuzzyDuplicates), AutonomyLevel::Suggest);
    assert_eq!(
        level(AutonomyTask::TitleAbstractScreening),
        AutonomyLevel::SecondReviewer
    );
    assert_eq!(
        level(AutonomyTask::FullTextScreening),
        AutonomyLevel::SecondReviewer
    );
    assert_eq!(level(AutonomyTask::Extraction), AutonomyLevel::Act);
    assert_eq!(level(AutonomyTask::Appraisal), AutonomyLevel::Suggest);

    let actor = Actor::new(ActorKind::User, "tester").expect("actor");
    set_autonomy_level(
        &pool,
        project_id,
        AutonomyTask::Extraction,
        AutonomyLevel::Off,
        &actor,
    )
    .await
    .expect("set");
    assert_eq!(
        resolve_autonomy_level(&pool, project_id, AutonomyTask::Extraction)
            .await
            .expect("level"),
        AutonomyLevel::Off
    );
    // The AI never finalizes screening alone.
    assert!(
        set_autonomy_level(
            &pool,
            project_id,
            AutonomyTask::TitleAbstractScreening,
            AutonomyLevel::Act,
            &actor
        )
        .await
        .is_err()
    );
    // Locked work cannot be stored even by writing the table directly.
    for task in ["protocol_publishing", "final_exclusion"] {
        let result = sqlx::query(
            "INSERT INTO project_ai_autonomy (project_id,task,level,updated_by_kind,updated_by_id)
             VALUES ($1,$2,'act','user','x')",
        )
        .bind(project_id)
        .bind(task)
        .execute(&pool)
        .await;
        assert!(result.is_err(), "{task} must not be storable");
    }
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn second_reviewer_disagreement_becomes_a_conflict_until_a_person_resolves_it() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool).await;
    let report_id = Uuid::new_v4();
    sqlx::query("INSERT INTO reports (id,title) VALUES ($1,'Conflict report')")
        .bind(report_id)
        .execute(&pool)
        .await
        .expect("report");
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project_id)
        .bind(report_id)
        .execute(&pool)
        .await
        .expect("membership");
    let mut tx = pool.begin().await.expect("tx");
    insert_reviewer_decision_in_transaction(
        &mut tx,
        &NewReviewerDecision {
            id: Uuid::new_v4(),
            project_id,
            report_id,
            stage: "title_abstract".to_owned(),
            decision: "include".to_owned(),
            rationale: "Matches the population.".to_owned(),
            evidence: serde_json::json!([{"label": "Abstract", "quote": "adults with asthma"}]),
            source: "ai",
            proposal_id: None,
            ai_run_id: None,
            model: Some("test".to_owned()),
            prompt_version: None,
            activity_id: None,
        },
    )
    .await
    .expect("insert");
    tx.commit().await.expect("commit");
    let rows = list_reviewer_decisions(&pool, project_id, None, None, 10)
        .await
        .expect("list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "waiting");

    // A human screening state that disagrees creates a conflict.
    sqlx::query(
        "INSERT INTO screening_state (project_id,report_id,title_abstract_status,final_status,revision)
         VALUES ($1,$2,'exclude','exclude',1)",
    )
    .bind(project_id)
    .bind(report_id)
    .execute(&pool)
    .await
    .expect("state");
    let rows = list_reviewer_decisions(&pool, project_id, None, Some("conflict"), 10)
        .await
        .expect("list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].human_decision.as_deref(), Some("exclude"));

    // Without a published protocol a person cannot record the final decision.
    let actor = Actor::new(ActorKind::User, "tester").expect("actor");
    let error = resolve_reviewer_conflict(
        &pool,
        project_id,
        rows[0].id,
        ResolveConflict {
            decision: deepref_domain::ScreeningDecision::Include,
            exclusion_reason_id: None,
            note: None,
        },
        &actor,
    )
    .await;
    assert!(matches!(
        error,
        Err(deepref_postgres::ReviewerError::NoProtocol)
    ));
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("cleanup");
}
