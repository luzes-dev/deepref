use std::collections::BTreeMap;

use deepref_application::{
    ProtocolCriterionCommand, PublishProtocolCommand, SaveProtocolDraftCommand,
};
use deepref_domain::{CriterionDimension, CriterionKind, CriterionStage, FrameworkKind, ProjectId};
use deepref_postgres::{
    ProtocolActor, ProtocolError, get_protocol_editor, get_published_protocol, migrate,
    publish_protocol, save_protocol_draft,
};
use sqlx::{AssertSqlSafe, Connection, PgConnection, PgPool, Row, postgres::PgPoolOptions};
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .ok()?;
    migrate(&pool).await.ok()?;
    Some(pool)
}

async fn project(pool: &PgPool, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,$2)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await
        .expect("project should be inserted");
    id
}

fn fields() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "population".to_owned(),
            "Adults with condition X".to_owned(),
        ),
        ("intervention".to_owned(), "Intervention Y".to_owned()),
        ("outcome".to_owned(), "Outcome Z".to_owned()),
    ])
}

fn criteria() -> Vec<ProtocolCriterionCommand> {
    vec![
        ProtocolCriterionCommand {
            id: None,
            kind: CriterionKind::Inclusion,
            stage: CriterionStage::Both,
            dimension: CriterionDimension::Population,
            label: "Population".to_owned(),
            description: "Adults with condition X".to_owned(),
        },
        ProtocolCriterionCommand {
            id: None,
            kind: CriterionKind::Exclusion,
            stage: CriterionStage::FullText,
            dimension: CriterionDimension::Language,
            label: "Language".to_owned(),
            description: "Exclude studies without an eligible translation".to_owned(),
        },
    ]
}

fn command(project_id: Uuid, expected_revision: i64) -> SaveProtocolDraftCommand {
    SaveProtocolDraftCommand {
        project_id: ProjectId::from(project_id),
        protocol_version_id: None,
        name: "Condition X protocol".to_owned(),
        objective: "Assess intervention Y".to_owned(),
        question: "Does intervention Y improve outcome Z?".to_owned(),
        framework_kind: FrameworkKind::Pico,
        framework_fields: fields(),
        criteria: criteria(),
        expected_revision,
    }
}

fn actor() -> ProtocolActor {
    ProtocolActor {
        kind: "user".to_owned(),
        id: "protocol-test-user".to_owned(),
    }
}

async fn save_initial(pool: &PgPool, project_id: Uuid) -> deepref_postgres::ProtocolDocument {
    save_protocol_draft(pool, &command(project_id, 0), &actor())
        .await
        .expect("initial draft should save")
}

async fn publish_initial(pool: &PgPool, project_id: Uuid) -> deepref_postgres::ProtocolDocument {
    let draft = save_initial(pool, project_id).await;
    publish_protocol(
        pool,
        &PublishProtocolCommand {
            project_id: ProjectId::from(project_id),
            protocol_version_id: draft.id,
            expected_revision: draft.revision,
        },
        &actor(),
    )
    .await
    .expect("initial protocol should publish")
}

#[tokio::test]
async fn initial_draft_publish_and_immutable_amendment_are_versioned() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool, "protocol lifecycle").await;
    let published_v1 = publish_initial(&pool, project_id).await;
    assert_eq!(published_v1.version, 1);
    assert_eq!(
        published_v1.status,
        deepref_domain::ProtocolStatus::Published
    );

    let mut amendment = command(project_id, published_v1.revision);
    amendment.protocol_version_id = Some(published_v1.id);
    amendment.name = "Condition X amended protocol".to_owned();
    let draft_v2 = save_protocol_draft(&pool, &amendment, &actor())
        .await
        .expect("published protocol should create an amendment draft");
    assert_eq!(draft_v2.version, 2);
    assert_eq!(draft_v2.amendment_of, Some(published_v1.id));
    assert_eq!(draft_v2.status, deepref_domain::ProtocolStatus::Draft);

    let published_v2 = publish_protocol(
        &pool,
        &PublishProtocolCommand {
            project_id: ProjectId::from(project_id),
            protocol_version_id: draft_v2.id,
            expected_revision: draft_v2.revision,
        },
        &actor(),
    )
    .await
    .expect("amendment should publish");
    assert_eq!(published_v2.version, 2);
    assert_eq!(published_v2.name, "Condition X amended protocol");

    let old = sqlx::query("SELECT status,name FROM protocol_versions WHERE id=$1")
        .bind(published_v1.id)
        .fetch_one(&pool)
        .await
        .expect("superseded version should remain queryable");
    assert_eq!(old.get::<String, _>("status"), "superseded");
    assert_eq!(old.get::<String, _>("name"), "Condition X protocol");
    assert_eq!(
        get_published_protocol(&pool, project_id).await.unwrap().id,
        published_v2.id
    );
    assert_eq!(
        get_protocol_editor(&pool, project_id).await.unwrap().id,
        published_v2.id
    );
}

#[tokio::test]
async fn draft_revision_conflict_has_no_partial_writes() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool, "protocol revision").await;
    let draft = save_initial(&pool, project_id).await;
    let mut stale = command(project_id, 0);
    stale.protocol_version_id = Some(draft.id);
    stale.name = "must not be written".to_owned();
    let error = save_protocol_draft(&pool, &stale, &actor())
        .await
        .expect_err("stale draft should conflict");
    assert!(matches!(error, ProtocolError::Conflict { .. }));
    let current = get_protocol_editor(&pool, project_id).await.unwrap();
    assert_eq!(current.name, "Condition X protocol");
    assert_eq!(current.revision, 1);
    assert_eq!(current.criteria.len(), 2);
}

#[tokio::test]
async fn existing_draft_rejects_criterion_ids_from_another_protocol_atomically() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool, "protocol criterion ownership").await;
    let other_project_id = project(&pool, "protocol criterion ownership other").await;
    let draft = save_initial(&pool, project_id).await;
    let other_draft = save_initial(&pool, other_project_id).await;

    let mut invalid = command(project_id, draft.revision);
    invalid.protocol_version_id = Some(draft.id);
    invalid.criteria[0].id = Some(other_draft.criteria[0].id);
    let error = save_protocol_draft(&pool, &invalid, &actor())
        .await
        .expect_err("criterion ids must remain scoped to one protocol");
    assert!(
        matches!(error, ProtocolError::Invalid(message) if message.contains("another protocol"))
    );

    let current = get_protocol_editor(&pool, project_id).await.unwrap();
    assert_eq!(current.revision, draft.revision);
    assert_eq!(current.criteria, draft.criteria);
}

#[tokio::test]
async fn invalid_framework_and_criteria_are_rejected_before_storage() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool, "protocol validation").await;
    let mut invalid_framework = command(project_id, 0);
    invalid_framework
        .framework_fields
        .insert("unknown".to_owned(), "value".to_owned());
    assert!(matches!(
        save_protocol_draft(&pool, &invalid_framework, &actor()).await,
        Err(ProtocolError::Invalid(_))
    ));

    let mut invalid_criteria = command(project_id, 0);
    invalid_criteria.criteria[0].label = " ".to_owned();
    assert!(matches!(
        save_protocol_draft(&pool, &invalid_criteria, &actor()).await,
        Err(ProtocolError::Invalid(_))
    ));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM protocol_versions WHERE project_id=$1")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn publish_writes_atomic_audit_and_preserves_historical_screening_reference() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool, "protocol audit").await;
    let published_v1 = publish_initial(&pool, project_id).await;
    let report_id = Uuid::new_v4();
    sqlx::query("INSERT INTO reports (id,title) VALUES ($1,'Historical report')")
        .bind(report_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project_id)
        .bind(report_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO screening_events (id,project_id,report_id,stage,decision,protocol_version_id,actor_kind,actor_id) VALUES ($1,$2,$3,'title_abstract','include',$4,'user','history-test')")
        .bind(Uuid::new_v4())
        .bind(project_id)
        .bind(report_id)
        .bind(published_v1.id)
        .execute(&pool)
        .await
        .unwrap();

    let mut amendment = command(project_id, published_v1.revision);
    amendment.protocol_version_id = Some(published_v1.id);
    let draft_v2 = save_protocol_draft(&pool, &amendment, &actor())
        .await
        .unwrap();
    publish_protocol(
        &pool,
        &PublishProtocolCommand {
            project_id: ProjectId::from(project_id),
            protocol_version_id: draft_v2.id,
            expected_revision: draft_v2.revision,
        },
        &actor(),
    )
    .await
    .unwrap();

    let audit: (i64, String) = sqlx::query_as(
        "SELECT count(*)::bigint, max(event_type) FROM review_events WHERE project_id=$1 AND event_type='protocol_published'",
    )
    .bind(project_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit.0, 2);
    assert_eq!(audit.1, "protocol_published");
    let historical: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM screening_events WHERE project_id=$1 AND protocol_version_id=$2",
    )
    .bind(project_id)
    .bind(published_v1.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(historical, 1);
}

#[tokio::test]
async fn protocol_reads_are_project_scoped_and_project_delete_cascades() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool, "protocol isolation").await;
    let other_project_id = project(&pool, "protocol isolation other").await;
    let published = publish_initial(&pool, project_id).await;
    let report_id = Uuid::new_v4();
    sqlx::query("INSERT INTO reports (id,title) VALUES ($1,'Cascade report')")
        .bind(report_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
        .bind(project_id)
        .bind(report_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO screening_events (id,project_id,report_id,stage,decision,protocol_version_id,actor_kind,actor_id) VALUES ($1,$2,$3,'title_abstract','include',$4,'user','cascade-test')")
        .bind(Uuid::new_v4())
        .bind(project_id)
        .bind(report_id)
        .bind(published.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        get_protocol_editor(&pool, other_project_id).await,
        Err(ProtocolError::NotFound)
    ));
    sqlx::query("DELETE FROM projects WHERE id=$1")
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM protocol_versions WHERE project_id=$1")
            .bind(project_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, 0);
    assert!(
        get_published_protocol(&pool, other_project_id)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn published_protocol_and_criteria_reject_direct_scientific_mutation() {
    let Some(pool) = database().await else { return };
    let project_id = project(&pool, "protocol immutable criteria").await;
    let published = publish_initial(&pool, project_id).await;
    let criterion_id = published.criteria[0].id;

    let update = sqlx::query("UPDATE eligibility_criteria SET label='tampered' WHERE id=$1")
        .bind(criterion_id)
        .execute(&pool)
        .await;
    assert!(update.is_err());

    let insert = sqlx::query(
        "INSERT INTO eligibility_criteria (id,protocol_version_id,criterion_type,stage,dimension,label,description,ordinal) VALUES ($1,$2,'include','both','other','tampered','tampered',99)",
    )
    .bind(Uuid::new_v4())
    .bind(published.id)
    .execute(&pool)
    .await;
    assert!(insert.is_err());

    let delete = sqlx::query("DELETE FROM eligibility_criteria WHERE id=$1")
        .bind(criterion_id)
        .execute(&pool)
        .await;
    assert!(delete.is_err());

    let protocol_update = sqlx::query("UPDATE protocol_versions SET name='tampered' WHERE id=$1")
        .bind(published.id)
        .execute(&pool)
        .await;
    assert!(protocol_update.is_err());

    let protocol_delete = sqlx::query("DELETE FROM protocol_versions WHERE id=$1")
        .bind(published.id)
        .execute(&pool)
        .await;
    assert!(protocol_delete.is_err());
}

#[tokio::test]
async fn migration_backfills_legacy_json_criteria_deterministically() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        return;
    };
    let schema = format!("protocol_migration_test_{}", Uuid::new_v4().simple());
    let mut connection = PgConnection::connect(&url)
        .await
        .expect("migration test database connection should work");
    sqlx::query(AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&mut connection)
        .await
        .expect("test schema should be created");
    sqlx::query(AssertSqlSafe(format!(
        "SET search_path TO {schema}, public"
    )))
    .execute(&mut connection)
    .await
    .expect("test schema should be selected");

    let result = async {
        deepref_postgres::MIGRATOR
            .run_to(10, &mut connection)
            .await
            .expect("migrations through PR5 should apply");
        let project_id = Uuid::new_v4();
        let protocol_id = Uuid::new_v4();
        sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'legacy backfill project')")
            .bind(project_id)
            .execute(&mut connection)
            .await
            .expect("legacy project should be inserted");
        sqlx::query(
            "INSERT INTO protocol_versions (id,project_id,version,name,status,criteria,published_at) VALUES ($1,$2,1,'Legacy','published',$3,now())",
        )
        .bind(protocol_id)
        .bind(project_id)
        .bind(serde_json::json!([
            {"id":"population","label":"Population","description":"People in scope"},
            {"id":"outcome","label":"Outcome","description":"Outcome in scope"}
        ]))
        .execute(&mut connection)
        .await
        .expect("legacy protocol should be inserted");
        deepref_postgres::MIGRATOR
            .run(&mut connection)
            .await
            .expect("PR6 migration should apply");

        let rows = sqlx::query(
            "SELECT id,ordinal,criterion_type,stage,dimension,label,description FROM eligibility_criteria WHERE protocol_version_id=$1 ORDER BY ordinal",
        )
        .bind(protocol_id)
        .fetch_all(&mut connection)
        .await
        .expect("backfilled criteria should be readable");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get::<i32, _>("ordinal"), 0);
        assert_eq!(rows[0].get::<String, _>("criterion_type"), "include");
        assert_eq!(rows[0].get::<String, _>("stage"), "both");
        assert_eq!(rows[0].get::<String, _>("dimension"), "population");
        assert_eq!(rows[1].get::<String, _>("dimension"), "outcome");

        let first_id = rows[0].get::<Uuid, _>("id");
        deepref_postgres::MIGRATOR
            .run(&mut connection)
            .await
            .expect("rerunning migrations should be a no-op");
        let second_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM eligibility_criteria WHERE protocol_version_id=$1 AND ordinal=0",
        )
        .bind(protocol_id)
        .fetch_one(&mut connection)
        .await
        .expect("backfilled criterion should remain");
        assert_eq!(first_id, second_id);

        let report_id = Uuid::new_v4();
        sqlx::query("INSERT INTO reports (id,title) VALUES ($1,'Legacy cascade report')")
            .bind(report_id)
            .execute(&mut connection)
            .await
            .expect("cascade report should be inserted");
        sqlx::query("INSERT INTO project_reports (project_id,report_id) VALUES ($1,$2)")
            .bind(project_id)
            .bind(report_id)
            .execute(&mut connection)
            .await
            .expect("cascade membership should be inserted");
        sqlx::query("INSERT INTO screening_events (id,project_id,report_id,stage,decision,protocol_version_id,actor_kind,actor_id) VALUES ($1,$2,$3,'title_abstract','include',$4,'user','migration-test')")
            .bind(Uuid::new_v4())
            .bind(project_id)
            .bind(report_id)
            .bind(protocol_id)
            .execute(&mut connection)
            .await
            .expect("historical screening event should be inserted");
        sqlx::query("DELETE FROM projects WHERE id=$1")
            .bind(project_id)
            .execute(&mut connection)
            .await
            .expect("project cascade should remove protocol history");
        let remaining_protocols: i64 =
            sqlx::query_scalar("SELECT count(*) FROM protocol_versions WHERE project_id=$1")
                .bind(project_id)
                .fetch_one(&mut connection)
                .await
                .expect("protocol count should be readable after cascade");
        assert_eq!(remaining_protocols, 0);
    };
    result.await;

    sqlx::query(AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(&mut connection)
        .await
        .expect("test schema should be removed");
}
