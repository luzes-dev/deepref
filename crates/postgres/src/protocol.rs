use std::collections::BTreeMap;

use deepref_application::{
    ProtocolCommandError, ProtocolCriterionCommand, PublishProtocolCommand,
    SaveProtocolDraftCommand,
};
use deepref_domain::{
    CriterionDimension, CriterionKind, CriterionStage, EligibilityCriterion, FrameworkKind,
    ProtocolFramework, ProtocolStatus, ProtocolValidationError, validate_criteria,
};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolActor {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolDocument {
    pub id: Uuid,
    pub project_id: Uuid,
    pub version: i32,
    pub name: String,
    pub status: ProtocolStatus,
    pub framework: ProtocolFramework,
    pub objective: String,
    pub question: String,
    pub criteria: Vec<EligibilityCriterion>,
    pub revision: i64,
    pub amendment_of: Option<Uuid>,
    pub published_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub created_by_kind: String,
    pub created_by_id: String,
    pub updated_by_kind: String,
    pub updated_by_id: String,
    pub published_by_kind: Option<String>,
    pub published_by_id: Option<String>,
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("project not found")]
    ProjectNotFound,
    #[error("protocol not found")]
    NotFound,
    #[error("protocol draft already exists")]
    DraftAlreadyExists,
    #[error("protocol version is not editable")]
    NotEditable,
    #[error("protocol revision conflict")]
    Conflict {
        code: &'static str,
        message: &'static str,
        current_revision: i64,
    },
    #[error("invalid protocol: {0}")]
    Invalid(String),
    #[error("protocol data integrity failure: {0}")]
    DataIntegrity(String),
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("serialization failed")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy)]
struct VersionState {
    id: Uuid,
    version: i32,
    status: ProtocolStatus,
    revision: i64,
    amendment_of: Option<Uuid>,
}

pub async fn get_protocol_editor(
    pool: &PgPool,
    project_id: Uuid,
) -> Result<ProtocolDocument, ProtocolError> {
    if !project_exists(pool, project_id).await? {
        return Err(ProtocolError::ProjectNotFound);
    }
    let row = sqlx::query!(
        r#"
        SELECT id
        FROM protocol_versions
        WHERE project_id = $1 AND status IN ('draft', 'published')
        ORDER BY CASE WHEN status = 'draft' THEN 0 ELSE 1 END, version DESC, id DESC
        LIMIT 1
        "#,
        project_id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(ProtocolError::NotFound)?;
    load_document(pool, project_id, row.id).await
}

pub async fn get_published_protocol(
    pool: &PgPool,
    project_id: Uuid,
) -> Result<ProtocolDocument, ProtocolError> {
    if !project_exists(pool, project_id).await? {
        return Err(ProtocolError::ProjectNotFound);
    }
    let row = sqlx::query!(
        "SELECT id FROM protocol_versions WHERE project_id=$1 AND status='published' ORDER BY version DESC, id DESC LIMIT 1",
        project_id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(ProtocolError::NotFound)?;
    load_document(pool, project_id, row.id).await
}

pub async fn save_protocol_draft(
    pool: &PgPool,
    command: &SaveProtocolDraftCommand,
    actor: &ProtocolActor,
) -> Result<ProtocolDocument, ProtocolError> {
    command
        .validate()
        .map_err(|error| ProtocolError::Invalid(error.to_string()))?;
    validate_actor(actor)?;

    let mut tx = pool.begin().await?;
    let lock_key = format!("protocol:{}", command.project_id.as_uuid());
    sqlx::query!(
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        lock_key,
    )
    .execute(&mut *tx)
    .await?;
    if !project_exists_tx(&mut tx, command.project_id.as_uuid()).await? {
        return Err(ProtocolError::ProjectNotFound);
    }

    let draft = current_version_tx(&mut tx, command.project_id.as_uuid(), "draft").await?;
    let published = current_version_tx(&mut tx, command.project_id.as_uuid(), "published").await?;

    let (version_id, version, revision, amendment_of, is_new) = match (draft, published) {
        (Some(draft), _) => {
            if command.protocol_version_id.is_some_and(|id| id != draft.id) {
                return Err(ProtocolError::Conflict {
                    code: "protocol_draft_changed",
                    message: "another protocol draft is current; refresh before saving",
                    current_revision: draft.revision,
                });
            }
            if command.expected_revision != draft.revision {
                return Err(ProtocolError::Conflict {
                    code: "protocol_revision_conflict",
                    message: "protocol draft changed; refresh before saving",
                    current_revision: draft.revision,
                });
            }
            (
                draft.id,
                draft.version,
                draft.revision + 1,
                draft.amendment_of,
                false,
            )
        }
        (None, Some(published)) => {
            if let Some(protocol_version_id) = command.protocol_version_id
                && protocol_version_id != published.id
            {
                return Err(ProtocolError::NotFound);
            }
            if command.expected_revision != published.revision {
                return Err(ProtocolError::Conflict {
                    code: "protocol_revision_conflict",
                    message: "published protocol changed; refresh before saving",
                    current_revision: published.revision,
                });
            }
            (
                Uuid::new_v4(),
                published.version + 1,
                1,
                Some(published.id),
                true,
            )
        }
        (None, None) => {
            if command.protocol_version_id.is_some() || command.expected_revision != 0 {
                return Err(ProtocolError::Conflict {
                    code: "protocol_revision_conflict",
                    message: "protocol has no current version; refresh before saving",
                    current_revision: 0,
                });
            }
            (Uuid::new_v4(), 1, 1, None, true)
        }
    };

    let criteria = build_criteria(&command.criteria, is_new)?;
    if !is_new {
        ensure_criterion_ids_belong_to_draft(&mut tx, version_id, &criteria).await?;
    }
    let framework =
        ProtocolFramework::new(command.framework_kind, command.framework_fields.clone())
            .map_err(|error| ProtocolError::Invalid(error.to_string()))?;
    let criteria_json = serde_json::to_value(&criteria)?;
    let framework_fields = serde_json::to_value(&framework.fields)?;

    let proj_uuid = command.project_id.as_uuid();
    let fw_kind = framework_kind_string(command.framework_kind);
    if is_new {
        let result = sqlx::query!(
            r#"
            INSERT INTO protocol_versions (
              id, project_id, version, name, status, criteria,
              framework_kind, framework_fields, objective, question, revision,
              amendment_of, created_by_kind, created_by_id, updated_by_kind, updated_by_id
            ) VALUES ($1,$2,$3,$4,'draft',$5,$6,$7,$8,$9,$10,$11,$12,$13,$12,$13)
            "#,
            version_id,
            proj_uuid,
            version,
            command.name,
            criteria_json,
            fw_kind,
            framework_fields,
            command.objective,
            command.question,
            revision,
            amendment_of,
            actor.kind,
            actor.id,
        )
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() != 1 {
            return Err(ProtocolError::DataIntegrity(
                "protocol draft insert did not affect one row".to_owned(),
            ));
        }
    } else {
        let result = sqlx::query!(
            r#"
            UPDATE protocol_versions
            SET name=$3, criteria=$4, framework_kind=$5, framework_fields=$6,
                objective=$7, question=$8, revision=$9, updated_at=now(),
                updated_by_kind=$10, updated_by_id=$11
            WHERE project_id=$1 AND id=$2 AND status='draft'
            "#,
            proj_uuid,
            version_id,
            command.name,
            criteria_json,
            fw_kind,
            framework_fields,
            command.objective,
            command.question,
            revision,
            actor.kind,
            actor.id,
        )
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() != 1 {
            return Err(ProtocolError::DataIntegrity(
                "protocol draft update did not affect one row".to_owned(),
            ));
        }
    }
    replace_criteria(&mut tx, version_id, &criteria).await?;
    tx.commit().await?;
    load_document(pool, command.project_id.as_uuid(), version_id).await
}

pub async fn publish_protocol(
    pool: &PgPool,
    command: &PublishProtocolCommand,
    actor: &ProtocolActor,
) -> Result<ProtocolDocument, ProtocolError> {
    command
        .validate()
        .map_err(|error| ProtocolError::Invalid(error.to_string()))?;
    validate_actor(actor)?;
    let mut tx = pool.begin().await?;
    let lock_key = format!("protocol:{}", command.project_id.as_uuid());
    sqlx::query!(
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        lock_key,
    )
    .execute(&mut *tx)
    .await?;
    let proj_uuid = command.project_id.as_uuid();
    if !project_exists_tx(&mut tx, proj_uuid).await? {
        return Err(ProtocolError::ProjectNotFound);
    }
    let Some(draft) = version_by_id_tx(&mut tx, proj_uuid, command.protocol_version_id).await?
    else {
        return Err(ProtocolError::NotFound);
    };
    if draft.status != ProtocolStatus::Draft {
        return Err(ProtocolError::NotEditable);
    }
    if draft.revision != command.expected_revision {
        return Err(ProtocolError::Conflict {
            code: "protocol_revision_conflict",
            message: "protocol draft changed; refresh before publishing",
            current_revision: draft.revision,
        });
    }

    sqlx::query!(
        "UPDATE protocol_versions SET status='superseded', updated_at=now(), updated_by_kind=$2, updated_by_id=$3 WHERE project_id=$1 AND status='published'",
        proj_uuid,
        actor.kind,
        actor.id,
    )
    .execute(&mut *tx)
    .await?;
    let publish_result = sqlx::query!(
        "UPDATE protocol_versions SET status='published', published_at=now(), published_by_kind=$2, published_by_id=$3, updated_at=now(), updated_by_kind=$2, updated_by_id=$3 WHERE project_id=$1 AND id=$4",
        proj_uuid,
        actor.kind,
        actor.id,
        command.protocol_version_id,
    )
    .execute(&mut *tx)
    .await?;
    if publish_result.rows_affected() != 1 {
        return Err(ProtocolError::DataIntegrity(
            "protocol publication did not affect one row".to_owned(),
        ));
    }
    let event_id = Uuid::new_v4();
    let payload = json!({
        "protocol_version_id": command.protocol_version_id,
        "expected_revision": command.expected_revision,
        "version": draft.version,
        "amendment_of": draft.amendment_of,
    });
    sqlx::query!(
        "INSERT INTO review_events (id,project_id,event_type,aggregate_type,aggregate_id,payload,actor_kind,actor_id) VALUES ($1,$2,'protocol_published','protocol_version',$3,$4,$5,$6)",
        event_id,
        proj_uuid,
        command.protocol_version_id,
        payload,
        actor.kind,
        actor.id,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    load_document(pool, proj_uuid, command.protocol_version_id).await
}

async fn project_exists(pool: &PgPool, project_id: Uuid) -> Result<bool, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1) as "exists!""#,
        project_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(exists)
}

async fn project_exists_tx(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1) as "exists!""#,
        project_id,
    )
    .fetch_one(&mut **tx)
    .await?;
    Ok(exists)
}

async fn current_version_tx(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    status: &str,
) -> Result<Option<VersionState>, ProtocolError> {
    let row = sqlx::query!(
        "SELECT id,version,status,revision,amendment_of FROM protocol_versions WHERE project_id=$1 AND status=$2 ORDER BY version DESC, id DESC LIMIT 1",
        project_id,
        status,
    )
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let status = protocol_status_from_string(row.status)?;
    Ok(Some(VersionState {
        id: row.id,
        version: row.version,
        status,
        revision: row.revision,
        amendment_of: row.amendment_of,
    }))
}

async fn version_by_id_tx(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    id: Uuid,
) -> Result<Option<VersionState>, ProtocolError> {
    let row = sqlx::query!(
        "SELECT id,version,status,revision,amendment_of FROM protocol_versions WHERE project_id=$1 AND id=$2 FOR UPDATE",
        project_id,
        id,
    )
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let status = protocol_status_from_string(row.status)?;
    Ok(Some(VersionState {
        id: row.id,
        version: row.version,
        status,
        revision: row.revision,
        amendment_of: row.amendment_of,
    }))
}

async fn load_document(
    pool: &PgPool,
    project_id: Uuid,
    id: Uuid,
) -> Result<ProtocolDocument, ProtocolError> {
    let row = sqlx::query!(
        r#"
        SELECT id, project_id, version, name, status, framework_kind,
               framework_fields, objective, question, revision, amendment_of,
               published_at, created_at, updated_at, created_by_kind, created_by_id,
               updated_by_kind, updated_by_id, published_by_kind, published_by_id
        FROM protocol_versions
        WHERE project_id=$1 AND id=$2
        "#,
        project_id,
        id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(ProtocolError::NotFound)?;
    let criterion_rows = sqlx::query!(
        "SELECT id,criterion_type,stage,dimension,label,description,ordinal FROM eligibility_criteria WHERE protocol_version_id=$1 ORDER BY ordinal,id",
        id,
    )
    .fetch_all(pool)
    .await?;
    let criteria = criterion_rows
        .into_iter()
        .map(|row| {
            EligibilityCriterion::new(
                row.id,
                criterion_kind_from_string(row.criterion_type)?,
                criterion_stage_from_string(row.stage)?,
                criterion_dimension_from_string(row.dimension)?,
                row.label,
                row.description,
                row.ordinal,
            )
            .map_err(ProtocolError::from)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let kind = framework_kind_from_string(row.framework_kind)?;
    let fields: BTreeMap<String, String> = serde_json::from_value(row.framework_fields)?;
    let framework = ProtocolFramework::new(kind, fields)
        .map_err(|error| ProtocolError::DataIntegrity(error.to_string()))?;
    Ok(ProtocolDocument {
        id: row.id,
        project_id: row.project_id,
        version: row.version,
        name: row.name,
        status: protocol_status_from_string(row.status)?,
        framework,
        objective: row.objective,
        question: row.question,
        criteria,
        revision: row.revision,
        amendment_of: row.amendment_of,
        published_at: row.published_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
        created_by_kind: row.created_by_kind,
        created_by_id: row.created_by_id,
        updated_by_kind: row.updated_by_kind,
        updated_by_id: row.updated_by_id,
        published_by_kind: row.published_by_kind,
        published_by_id: row.published_by_id,
    })
}

fn build_criteria(
    commands: &[ProtocolCriterionCommand],
    new_version: bool,
) -> Result<Vec<EligibilityCriterion>, ProtocolError> {
    let criteria = commands
        .iter()
        .enumerate()
        .map(|(ordinal, criterion)| {
            EligibilityCriterion::new(
                if new_version {
                    Uuid::new_v4()
                } else {
                    criterion.id.unwrap_or_else(Uuid::new_v4)
                },
                criterion.kind,
                criterion.stage,
                criterion.dimension,
                criterion.label.clone(),
                criterion.description.clone(),
                i32::try_from(ordinal).map_err(|_| ProtocolValidationError::TooManyCriteria)?,
            )
            .map_err(ProtocolError::from)
        })
        .collect::<Result<Vec<_>, _>>()?;
    validate_criteria(&criteria).map_err(ProtocolError::from)
}

async fn replace_criteria(
    tx: &mut Transaction<'_, Postgres>,
    protocol_version_id: Uuid,
    criteria: &[EligibilityCriterion],
) -> Result<(), ProtocolError> {
    sqlx::query!(
        "DELETE FROM eligibility_criteria WHERE protocol_version_id=$1",
        protocol_version_id,
    )
    .execute(&mut **tx)
    .await?;
    for criterion in criteria {
        let crit_kind = criterion_kind_string(criterion.kind);
        let crit_stage = criterion_stage_string(criterion.stage);
        let crit_dim = criterion_dimension_string(criterion.dimension);
        sqlx::query!(
            "INSERT INTO eligibility_criteria (id,protocol_version_id,criterion_type,stage,dimension,label,description,ordinal) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
            criterion.id,
            protocol_version_id,
            crit_kind,
            crit_stage,
            crit_dim,
            criterion.label,
            criterion.description,
            criterion.ordinal,
        )
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn ensure_criterion_ids_belong_to_draft(
    tx: &mut Transaction<'_, Postgres>,
    protocol_version_id: Uuid,
    criteria: &[EligibilityCriterion],
) -> Result<(), ProtocolError> {
    let ids: Vec<Uuid> = criteria.iter().map(|criterion| criterion.id).collect();
    if ids.is_empty() {
        return Ok(());
    }
    let foreign_id = sqlx::query_scalar!(
        "SELECT id FROM eligibility_criteria WHERE id = ANY($1::uuid[]) AND protocol_version_id <> $2 LIMIT 1",
        &ids as &[Uuid],
        protocol_version_id,
    )
    .fetch_optional(&mut **tx)
    .await?;
    if foreign_id.is_some() {
        return Err(ProtocolError::Invalid(
            "eligibility criterion id belongs to another protocol".to_owned(),
        ));
    }
    Ok(())
}

fn validate_actor(actor: &ProtocolActor) -> Result<(), ProtocolError> {
    if !matches!(actor.kind.as_str(), "user" | "automation" | "system") {
        return Err(ProtocolError::Invalid(
            "actor kind is not supported".to_owned(),
        ));
    }
    if actor.id.trim().is_empty() {
        return Err(ProtocolError::Invalid(
            "actor id must not be blank".to_owned(),
        ));
    }
    Ok(())
}

fn protocol_status_from_string(value: String) -> Result<ProtocolStatus, ProtocolError> {
    match value.as_str() {
        "draft" => Ok(ProtocolStatus::Draft),
        "published" => Ok(ProtocolStatus::Published),
        "superseded" => Ok(ProtocolStatus::Superseded),
        other => Err(ProtocolError::DataIntegrity(format!(
            "unknown protocol status {other}"
        ))),
    }
}

fn framework_kind_string(kind: FrameworkKind) -> &'static str {
    match kind {
        FrameworkKind::Pico => "pico",
        FrameworkKind::Picos => "picos",
        FrameworkKind::Peco => "peco",
        FrameworkKind::Peo => "peo",
        FrameworkKind::Pcc => "pcc",
        FrameworkKind::Spider => "spider",
        FrameworkKind::Custom => "custom",
    }
}

fn framework_kind_from_string(value: String) -> Result<FrameworkKind, ProtocolError> {
    match value.as_str() {
        "pico" => Ok(FrameworkKind::Pico),
        "picos" => Ok(FrameworkKind::Picos),
        "peco" => Ok(FrameworkKind::Peco),
        "peo" => Ok(FrameworkKind::Peo),
        "pcc" => Ok(FrameworkKind::Pcc),
        "spider" => Ok(FrameworkKind::Spider),
        "custom" => Ok(FrameworkKind::Custom),
        other => Err(ProtocolError::DataIntegrity(format!(
            "unknown framework kind {other}"
        ))),
    }
}

fn criterion_kind_string(kind: CriterionKind) -> &'static str {
    match kind {
        CriterionKind::Inclusion => "include",
        CriterionKind::Exclusion => "exclude",
    }
}

fn criterion_kind_from_string(value: String) -> Result<CriterionKind, ProtocolError> {
    match value.as_str() {
        "include" => Ok(CriterionKind::Inclusion),
        "exclude" => Ok(CriterionKind::Exclusion),
        other => Err(ProtocolError::DataIntegrity(format!(
            "unknown criterion kind {other}"
        ))),
    }
}

fn criterion_stage_string(stage: CriterionStage) -> &'static str {
    match stage {
        CriterionStage::TitleAbstract => "title_abstract",
        CriterionStage::FullText => "full_text",
        CriterionStage::Both => "both",
    }
}

fn criterion_stage_from_string(value: String) -> Result<CriterionStage, ProtocolError> {
    match value.as_str() {
        "title_abstract" => Ok(CriterionStage::TitleAbstract),
        "full_text" => Ok(CriterionStage::FullText),
        "both" => Ok(CriterionStage::Both),
        other => Err(ProtocolError::DataIntegrity(format!(
            "unknown criterion stage {other}"
        ))),
    }
}

fn criterion_dimension_string(dimension: CriterionDimension) -> &'static str {
    match dimension {
        CriterionDimension::Population => "population",
        CriterionDimension::Intervention => "intervention",
        CriterionDimension::Comparator => "comparator",
        CriterionDimension::Outcome => "outcome",
        CriterionDimension::Design => "design",
        CriterionDimension::Setting => "setting",
        CriterionDimension::Language => "language",
        CriterionDimension::Date => "date",
        CriterionDimension::Other => "other",
    }
}

fn criterion_dimension_from_string(value: String) -> Result<CriterionDimension, ProtocolError> {
    match value.as_str() {
        "population" => Ok(CriterionDimension::Population),
        "intervention" => Ok(CriterionDimension::Intervention),
        "comparator" => Ok(CriterionDimension::Comparator),
        "outcome" => Ok(CriterionDimension::Outcome),
        "design" => Ok(CriterionDimension::Design),
        "setting" => Ok(CriterionDimension::Setting),
        "language" => Ok(CriterionDimension::Language),
        "date" => Ok(CriterionDimension::Date),
        "other" => Ok(CriterionDimension::Other),
        other => Err(ProtocolError::DataIntegrity(format!(
            "unknown criterion dimension {other}"
        ))),
    }
}

impl From<ProtocolValidationError> for ProtocolError {
    fn from(error: ProtocolValidationError) -> Self {
        Self::Invalid(error.to_string())
    }
}

impl From<ProtocolCommandError> for ProtocolError {
    fn from(error: ProtocolCommandError) -> Self {
        Self::Invalid(error.to_string())
    }
}
