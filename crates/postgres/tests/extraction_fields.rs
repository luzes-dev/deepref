#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::string_slice
)]

use deepref_application::{ExtractionFieldDefinition, ExtractionFieldType};
use deepref_domain::ProjectId;
use deepref_postgres::{
    ExtractionError, ExtractionFieldUpdate, create_field_definition, list_field_definitions,
    list_values, update_field_definition,
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

struct Seeded {
    project: ProjectId,
    study: Uuid,
    field: Uuid,
}

async fn seed(pool: &PgPool) -> Seeded {
    let project = ProjectId::new(Uuid::new_v4());
    let study = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id,name) VALUES ($1,'field edit test')")
        .bind(project.as_uuid())
        .execute(pool)
        .await
        .expect("project inserts");
    sqlx::query(
        "INSERT INTO studies
         (id,project_id,title,design_context,study_revision,updated_by_actor_kind,updated_by_actor_id)
         VALUES ($1,$2,'Field study','{}'::jsonb,0,'user','fixture')",
    )
    .bind(study)
    .bind(project.as_uuid())
    .execute(pool)
    .await
    .expect("study inserts");
    let field = Uuid::new_v4();
    create_field_definition(
        pool,
        ExtractionFieldDefinition {
            id: field,
            project_id: project,
            version: 1,
            field_key: "duracao_da_intervencao".to_owned(),
            label: "Duração da intervenção".to_owned(),
            value_type: ExtractionFieldType::Text,
            required: false,
        },
    )
    .await
    .expect("field definition is created");
    Seeded {
        project,
        study,
        field,
    }
}

/// Records a text value for version 1 of the field. The value has no report
/// or source block, which the schema allows for reviewer-entered values.
async fn record_text_value(pool: &PgPool, seeded: &Seeded) {
    sqlx::query(
        "INSERT INTO extraction_values
         (id,project_id,study_id,field_definition_id,field_definition_version,value_type,
          text_value,approved_by_actor_kind,approved_by_actor_id)
         VALUES (gen_random_uuid(),$1,$2,$3,1,'text','Doze semanas','user','fixture')",
    )
    .bind(seeded.project.as_uuid())
    .bind(seeded.study)
    .bind(seeded.field)
    .execute(pool)
    .await
    .expect("value inserts");
}

async fn field_versions(pool: &PgPool, seeded: &Seeded) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM extraction_field_definitions WHERE project_id=$1 AND id=$2",
    )
    .bind(seeded.project.as_uuid())
    .bind(seeded.field)
    .fetch_one(pool)
    .await
    .expect("version count runs")
}

#[tokio::test]
async fn label_and_required_edits_stay_on_the_same_version_and_keep_values() {
    let Some(pool) = database().await else {
        return;
    };
    let seeded = seed(&pool).await;
    record_text_value(&pool, &seeded).await;

    let updated = update_field_definition(
        &pool,
        seeded.project.as_uuid(),
        seeded.field,
        ExtractionFieldUpdate {
            label: "Duração (semanas)".to_owned(),
            value_type: ExtractionFieldType::Text,
            required: true,
        },
    )
    .await
    .expect("label and required edit succeeds");

    assert_eq!(updated.version, 1, "label edits do not bump the version");
    assert_eq!(
        updated.field_key, "duracao_da_intervencao",
        "keys never change"
    );
    assert_eq!(updated.label, "Duração (semanas)");
    assert!(updated.required);
    assert_eq!(field_versions(&pool, &seeded).await, 1);

    let fields = list_field_definitions(&pool, seeded.project.as_uuid())
        .await
        .expect("fields list");
    let field = fields
        .iter()
        .find(|field| field.id == seeded.field)
        .expect("edited field is listed");
    assert_eq!(field.version, 1);
    assert_eq!(field.label, "Duração (semanas)");
    assert!(field.required);

    let values = list_values(&pool, seeded.project.as_uuid(), seeded.study)
        .await
        .expect("values list");
    assert_eq!(values.len(), 1, "the recorded value stays visible");
    assert_eq!(values[0].field_definition_id, seeded.field);
    assert_eq!(values[0].field_definition_version, 1);
}

#[tokio::test]
async fn value_type_change_is_refused_while_the_field_has_values() {
    let Some(pool) = database().await else {
        return;
    };
    let seeded = seed(&pool).await;
    record_text_value(&pool, &seeded).await;

    let refused = update_field_definition(
        &pool,
        seeded.project.as_uuid(),
        seeded.field,
        ExtractionFieldUpdate {
            label: "Duração da intervenção".to_owned(),
            value_type: ExtractionFieldType::Number,
            required: false,
        },
    )
    .await;

    assert!(
        matches!(refused, Err(ExtractionError::FieldHasValues)),
        "a field with values keeps its type, got {refused:?}"
    );
    let fields = list_field_definitions(&pool, seeded.project.as_uuid())
        .await
        .expect("fields list");
    let field = fields
        .iter()
        .find(|field| field.id == seeded.field)
        .expect("field is still listed");
    assert_eq!(field.value_type, ExtractionFieldType::Text);
    assert_eq!(field.version, 1);
}

#[tokio::test]
async fn value_type_change_without_values_becomes_the_next_version() {
    let Some(pool) = database().await else {
        return;
    };
    let seeded = seed(&pool).await;

    let updated = update_field_definition(
        &pool,
        seeded.project.as_uuid(),
        seeded.field,
        ExtractionFieldUpdate {
            label: "Duração da intervenção".to_owned(),
            value_type: ExtractionFieldType::Number,
            required: false,
        },
    )
    .await
    .expect("type change without values succeeds");

    assert_eq!(updated.version, 2, "a type change is recorded as version 2");
    assert_eq!(updated.id, seeded.field, "the field keeps its identity");
    assert_eq!(updated.field_key, "duracao_da_intervencao");
    assert_eq!(field_versions(&pool, &seeded).await, 2, "version 1 is kept");

    let fields = list_field_definitions(&pool, seeded.project.as_uuid())
        .await
        .expect("fields list");
    let field = fields
        .iter()
        .find(|field| field.id == seeded.field)
        .expect("field is listed");
    assert_eq!(field.version, 2);
    assert_eq!(field.value_type, ExtractionFieldType::Number);
}

#[tokio::test]
async fn editing_an_unknown_field_is_not_found() {
    let Some(pool) = database().await else {
        return;
    };
    let seeded = seed(&pool).await;

    let missing = update_field_definition(
        &pool,
        seeded.project.as_uuid(),
        Uuid::new_v4(),
        ExtractionFieldUpdate {
            label: "Nada".to_owned(),
            value_type: ExtractionFieldType::Text,
            required: false,
        },
    )
    .await;

    assert!(matches!(missing, Err(ExtractionError::DefinitionNotFound)));
}

#[tokio::test]
async fn a_blank_label_is_rejected_before_anything_changes() {
    let Some(pool) = database().await else {
        return;
    };
    let seeded = seed(&pool).await;

    let blank = update_field_definition(
        &pool,
        seeded.project.as_uuid(),
        seeded.field,
        ExtractionFieldUpdate {
            label: "   ".to_owned(),
            value_type: ExtractionFieldType::Text,
            required: true,
        },
    )
    .await;

    assert!(matches!(blank, Err(ExtractionError::InvalidDefinition(_))));
    let fields = list_field_definitions(&pool, seeded.project.as_uuid())
        .await
        .expect("fields list");
    let field = fields
        .iter()
        .find(|field| field.id == seeded.field)
        .expect("field is listed");
    assert_eq!(field.label, "Duração da intervenção");
    assert!(!field.required);
}
