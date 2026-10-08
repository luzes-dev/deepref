use chrono::{DateTime, NaiveDate, Utc};
use deepref_application::{
    ExtractionFieldDefinition, ExtractionFieldType, ExtractionValidationError, ExtractionValue,
};
use deepref_domain::{Actor, ProjectId};
use sqlx::{PgPool, Postgres, Row, Transaction};
use thiserror::Error;
use uuid::Uuid;

type DatabaseValue = (
    &'static str,
    Option<String>,
    Option<f64>,
    Option<bool>,
    Option<NaiveDate>,
);

#[derive(Debug, Clone, PartialEq)]
pub struct ExtractionValueRecord {
    pub id: Uuid,
    pub project_id: ProjectId,
    pub study_id: Uuid,
    pub report_id: Option<Uuid>,
    pub field_definition_id: Uuid,
    pub field_definition_version: i32,
    pub value: ExtractionValue,
    pub rationale: Option<String>,
    pub source_document_id: Option<Uuid>,
    pub source_block_id: Option<Uuid>,
    pub source_page: Option<i32>,
    pub source_parser_version: Option<String>,
    pub source_content_hash: Option<String>,
    pub approved_by_actor_kind: String,
    pub approved_by_actor_id: String,
    pub approved_at: DateTime<Utc>,
    /// True while an AI-entered value waits for a person to confirm it.
    pub needs_verification: bool,
    pub verified_at: Option<DateTime<Utc>>,
    pub verified_by: Option<String>,
}

/// How an extraction proposal is applied.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExtractionApplyOptions {
    /// Flag the stored values as "to verify" until a person confirms them.
    pub needs_verification: bool,
    /// Leave fields that already have a current value untouched.
    pub skip_existing: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ExtractionApplyOutcome {
    pub inserted_value_ids: Vec<Uuid>,
    pub skipped_existing: usize,
}

#[derive(Debug, Error)]
pub enum ExtractionError {
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("extraction field definition is invalid: {0}")]
    InvalidDefinition(String),
    #[error("extraction field definition version is immutable")]
    ImmutableDefinition,
    #[error("extraction field definition was not found")]
    DefinitionNotFound,
    #[error("extraction field definition is no longer the latest version for its field key")]
    StaleDefinitionVersion,
    #[error("study was not found in this project")]
    StudyNotFound,
    #[error("extraction value is invalid: {0}")]
    InvalidValue(String),
    #[error("extraction source block is not active or is outside the study")]
    EvidenceNotInStudy,
    #[error("required extraction field has insufficient evidence")]
    RequiredFieldInsufficient,
    #[error("an approved value already exists for this study field version")]
    ValueAlreadyApproved,
    #[error("the field already has values, so its value type cannot change; create a new field")]
    FieldHasValues,
}

pub async fn list_field_definitions(
    pool: &PgPool,
    project_id: Uuid,
) -> Result<Vec<ExtractionFieldDefinition>, ExtractionError> {
    let rows = sqlx::query(
        "SELECT id, project_id, version, field_key, label, value_type, required
         FROM (
           SELECT DISTINCT ON (field_key)
                  id, project_id, version, field_key, label, value_type, required,
                  min(created_at) OVER (PARTITION BY field_key) AS first_created_at
           FROM extraction_field_definitions
           WHERE project_id=$1
           ORDER BY field_key, version DESC, id
         ) latest
         ORDER BY first_created_at, field_key",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(field_from_row).collect()
}

pub async fn create_field_definition(
    pool: &PgPool,
    definition: ExtractionFieldDefinition,
) -> Result<ExtractionFieldDefinition, ExtractionError> {
    definition.validate().map_err(extraction_validation_error)?;
    let mut tx = pool.begin().await?;
    let project_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=$1)")
            .bind(definition.project_id.as_uuid())
            .fetch_one(&mut *tx)
            .await?;
    if !project_exists {
        return Err(ExtractionError::StudyNotFound);
    }
    let inserted = sqlx::query(
        "INSERT INTO extraction_field_definitions
         (id,project_id,version,field_key,label,value_type,required)
         VALUES ($1,$2,$3,$4,$5,$6,$7) ON CONFLICT DO NOTHING",
    )
    .bind(definition.id)
    .bind(definition.project_id.as_uuid())
    .bind(i32::try_from(definition.version).map_err(|_| {
        ExtractionError::InvalidDefinition("field definition version is too large".to_owned())
    })?)
    .bind(&definition.field_key)
    .bind(&definition.label)
    .bind(definition.value_type.as_str())
    .bind(definition.required)
    .execute(&mut *tx)
    .await?;
    if inserted.rows_affected() == 0 {
        let existing = sqlx::query(
            "SELECT id, project_id, version, field_key, label, value_type, required
             FROM extraction_field_definitions
             WHERE project_id=$1 AND id=$2 AND version=$3",
        )
        .bind(definition.project_id.as_uuid())
        .bind(definition.id)
        .bind(i32::try_from(definition.version).map_err(|_| {
            ExtractionError::InvalidDefinition("field definition version is too large".to_owned())
        })?)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ExtractionError::ImmutableDefinition)?;
        let existing = field_from_row(existing)?;
        if existing != definition {
            return Err(ExtractionError::ImmutableDefinition);
        }
    }
    tx.commit().await?;
    Ok(definition)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionFieldUpdate {
    pub label: String,
    pub value_type: ExtractionFieldType,
    pub required: bool,
}

/// Edits the label, value type or `required` flag of a field.
///
/// Label and `required` are changed on the latest version in place. Neither
/// one changes what a stored value means, and current values are unique per
/// (study, field, version), so bumping the version would split a field's values
/// across two versions. The value type does change how the typed columns are
/// read, so it is refused while the field has any value (current or
/// superseded). Without values, a type change becomes the next version of the
/// field, with the same id and key. Existing versions are never rewritten.
pub async fn update_field_definition(
    pool: &PgPool,
    project_id: Uuid,
    field_id: Uuid,
    update: ExtractionFieldUpdate,
) -> Result<ExtractionFieldDefinition, ExtractionError> {
    let mut tx = pool.begin().await?;
    let current = sqlx::query(
        "SELECT id, project_id, version, field_key, label, value_type, required
         FROM extraction_field_definitions
         WHERE project_id=$1 AND id=$2
         ORDER BY version DESC
         LIMIT 1
         FOR UPDATE",
    )
    .bind(project_id)
    .bind(field_id)
    .fetch_optional(&mut *tx)
    .await?
    .map(field_from_row)
    .transpose()?
    .ok_or(ExtractionError::DefinitionNotFound)?;
    let mut next = ExtractionFieldDefinition {
        id: current.id,
        project_id: current.project_id,
        version: current.version,
        field_key: current.field_key.clone(),
        label: update.label,
        value_type: update.value_type,
        required: update.required,
    };
    next.validate().map_err(extraction_validation_error)?;
    if next.value_type == current.value_type {
        if next.label != current.label || next.required != current.required {
            sqlx::query(
                "UPDATE extraction_field_definitions SET label=$4, required=$5
                 WHERE project_id=$1 AND id=$2 AND version=$3",
            )
            .bind(project_id)
            .bind(field_id)
            .bind(version_column(current.version)?)
            .bind(&next.label)
            .bind(next.required)
            .execute(&mut *tx)
            .await?;
        }
    } else {
        let has_values: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM extraction_values
             WHERE project_id=$1 AND field_definition_id=$2)",
        )
        .bind(project_id)
        .bind(field_id)
        .fetch_one(&mut *tx)
        .await?;
        if has_values {
            return Err(ExtractionError::FieldHasValues);
        }
        next.version = current.version.checked_add(1).ok_or_else(|| {
            ExtractionError::InvalidDefinition("field definition version is too large".to_owned())
        })?;
        sqlx::query(
            "INSERT INTO extraction_field_definitions
             (id,project_id,version,field_key,label,value_type,required)
             VALUES ($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(next.id)
        .bind(project_id)
        .bind(version_column(next.version)?)
        .bind(&next.field_key)
        .bind(&next.label)
        .bind(next.value_type.as_str())
        .bind(next.required)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(next)
}

fn version_column(version: u32) -> Result<i32, ExtractionError> {
    i32::try_from(version).map_err(|_| {
        ExtractionError::InvalidDefinition("field definition version is too large".to_owned())
    })
}

pub async fn list_values(
    pool: &PgPool,
    project_id: Uuid,
    study_id: Uuid,
) -> Result<Vec<ExtractionValueRecord>, ExtractionError> {
    let study_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM studies WHERE project_id=$1 AND id=$2)")
            .bind(project_id)
            .bind(study_id)
            .fetch_one(pool)
            .await?;
    if !study_exists {
        return Err(ExtractionError::StudyNotFound);
    }
    let rows = sqlx::query(
        "SELECT id,project_id,study_id,report_id,field_definition_id,
                field_definition_version,value_type,text_value,number_value,boolean_value,
                date_value,rationale,source_document_id,source_block_id,source_page,
                source_parser_version,source_content_hash,approved_by_actor_kind,
                approved_by_actor_id,approved_at,needs_verification,verified_at,
                verified_by_actor_id
         FROM extraction_values
         WHERE project_id=$1 AND study_id=$2 AND superseded_at IS NULL
         ORDER BY field_definition_id,field_definition_version,id",
    )
    .bind(project_id)
    .bind(study_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(value_from_row).collect()
}

/// Block a reviewer cites for a manually entered value.  Report, page, parser
/// version and content hash are derived from the block itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManualExtractionEvidence {
    pub document_id: Uuid,
    pub document_block_id: Uuid,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManualExtractionValue {
    pub field_id: Uuid,
    pub value: ExtractionValue,
    pub rationale: Option<String>,
    pub evidence: Option<ManualExtractionEvidence>,
}

/// Records a reviewer-entered value for the latest version of a field.  Any
/// current value is superseded (kept as history) rather than overwritten.
pub async fn record_manual_value(
    pool: &PgPool,
    project_id: Uuid,
    study_id: Uuid,
    input: ManualExtractionValue,
    actor: &Actor,
) -> Result<ExtractionValueRecord, ExtractionError> {
    let ManualExtractionValue {
        field_id,
        value,
        rationale,
        evidence,
    } = input;
    let mut tx = pool.begin().await?;
    ensure_study(&mut tx, project_id, study_id).await?;
    let definition_row = sqlx::query(
        "SELECT id,project_id,version,field_key,label,value_type,required
         FROM extraction_field_definitions
         WHERE project_id=$1 AND id=$2
         ORDER BY version DESC LIMIT 1",
    )
    .bind(project_id)
    .bind(field_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ExtractionError::DefinitionNotFound)?;
    let definition = field_from_row(definition_row)?;
    value
        .validate_for(&definition)
        .map_err(|error| ExtractionError::InvalidValue(error.to_string()))?;
    let (value_type, text_value, number_value, boolean_value, date_value) =
        database_value_from_domain(&value);
    let rationale = rationale
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty());
    if rationale.as_ref().is_some_and(|text| text.len() > 4_000) {
        return Err(ExtractionError::InvalidValue(
            "rationale must contain at most 4000 characters".to_owned(),
        ));
    }
    let version = i32::try_from(definition.version).map_err(|_| {
        ExtractionError::InvalidValue("field definition version is too large".to_owned())
    })?;
    let source = match evidence {
        None => None,
        Some(evidence) => {
            let row = sqlx::query(
                "SELECT d.report_id, b.page_number, b.parser_version, b.content_hash
                 FROM study_reports sr
                 JOIN documents d ON d.project_id=sr.project_id AND d.report_id=sr.report_id
                   AND d.id=$3
                 JOIN document_blocks b ON b.document_id=d.id AND b.id=$4
                 JOIN document_pages p ON p.document_id=d.id
                   AND p.parser_version=b.parser_version
                   AND p.page_number=b.page_number AND p.active
                 WHERE sr.project_id=$1 AND sr.study_id=$2
                   AND d.active_parser_version=b.parser_version AND b.active
                 LIMIT 1",
            )
            .bind(project_id)
            .bind(study_id)
            .bind(evidence.document_id)
            .bind(evidence.document_block_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ExtractionError::EvidenceNotInStudy)?;
            Some((
                row.get::<Uuid, _>("report_id"),
                row.get::<i32, _>("page_number"),
                row.get::<String, _>("parser_version"),
                row.get::<String, _>("content_hash"),
            ))
        }
    };
    sqlx::query(
        "UPDATE extraction_values
         SET superseded_at=now(), superseded_by_actor_kind=$5, superseded_by_actor_id=$6
         WHERE project_id=$1 AND study_id=$2 AND field_definition_id=$3
           AND field_definition_version=$4 AND superseded_at IS NULL",
    )
    .bind(project_id)
    .bind(study_id)
    .bind(definition.id)
    .bind(version)
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .execute(&mut *tx)
    .await?;
    let (report_id, source_page, parser_version, content_hash) = match source {
        Some((report, page, parser, hash)) => (Some(report), Some(page), Some(parser), Some(hash)),
        None => (None, None, None, None),
    };
    let row = sqlx::query(
        "INSERT INTO extraction_values
         (id,project_id,study_id,report_id,field_definition_id,field_definition_version,
          value_type,text_value,number_value,boolean_value,date_value,rationale,
          source_document_id,source_block_id,source_page,source_parser_version,
          source_content_hash,approved_by_actor_kind,approved_by_actor_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)
         RETURNING id,project_id,study_id,report_id,field_definition_id,
                   field_definition_version,value_type,text_value,number_value,boolean_value,
                   date_value,rationale,source_document_id,source_block_id,source_page,
                   source_parser_version,source_content_hash,approved_by_actor_kind,
                   approved_by_actor_id,approved_at,needs_verification,verified_at,
                   verified_by_actor_id",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(study_id)
    .bind(report_id)
    .bind(definition.id)
    .bind(version)
    .bind(value_type)
    .bind(text_value)
    .bind(number_value)
    .bind(boolean_value)
    .bind(date_value)
    .bind(rationale)
    .bind(evidence.map(|evidence| evidence.document_id))
    .bind(evidence.map(|evidence| evidence.document_block_id))
    .bind(source_page)
    .bind(parser_version)
    .bind(content_hash)
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .fetch_one(&mut *tx)
    .await
    .map_err(|error| {
        if is_unique_violation(&error) {
            ExtractionError::ValueAlreadyApproved
        } else {
            ExtractionError::Database(error)
        }
    })?;
    tx.commit().await?;
    value_from_row(row)
}

/// Clears the current value of a field; the cleared value stays as history.
/// Clearing a field that has no value is a no-op.
pub async fn clear_value(
    pool: &PgPool,
    project_id: Uuid,
    study_id: Uuid,
    field_id: Uuid,
    actor: &Actor,
) -> Result<(), ExtractionError> {
    let mut tx = pool.begin().await?;
    ensure_study(&mut tx, project_id, study_id).await?;
    let defined: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM extraction_field_definitions WHERE project_id=$1 AND id=$2)",
    )
    .bind(project_id)
    .bind(field_id)
    .fetch_one(&mut *tx)
    .await?;
    if !defined {
        return Err(ExtractionError::DefinitionNotFound);
    }
    sqlx::query(
        "UPDATE extraction_values
         SET superseded_at=now(), superseded_by_actor_kind=$4, superseded_by_actor_id=$5
         WHERE project_id=$1 AND study_id=$2 AND field_definition_id=$3
           AND superseded_at IS NULL",
    )
    .bind(project_id)
    .bind(study_id)
    .bind(field_id)
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn ensure_study(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    study_id: Uuid,
) -> Result<(), ExtractionError> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM studies WHERE project_id=$1 AND id=$2)")
            .bind(project_id)
            .bind(study_id)
            .fetch_one(&mut **tx)
            .await?;
    if exists {
        Ok(())
    } else {
        Err(ExtractionError::StudyNotFound)
    }
}

fn database_value_from_domain(value: &ExtractionValue) -> DatabaseValue {
    match value {
        ExtractionValue::Text { value } => ("text", Some(value.clone()), None, None, None),
        ExtractionValue::Number { value } => ("number", None, Some(*value), None, None),
        ExtractionValue::Boolean { value } => ("boolean", None, None, Some(*value), None),
        ExtractionValue::Date { value } => ("date", None, None, None, Some(*value)),
    }
}

pub async fn apply_data_extraction_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
    study_id: Uuid,
    proposal_id: Uuid,
    extraction: &deepref_ai::DataExtraction,
    actor: &Actor,
    options: ExtractionApplyOptions,
) -> Result<ExtractionApplyOutcome, ExtractionError> {
    let mut outcome = ExtractionApplyOutcome::default();
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM studies WHERE project_id=$1 AND id=$2)")
            .bind(project_id.as_uuid())
            .bind(study_id)
            .fetch_one(&mut **tx)
            .await?;
    if !exists || extraction.study_id != study_id {
        return Err(ExtractionError::StudyNotFound);
    }

    let mut prepared = Vec::with_capacity(extraction.fields.len());
    let mut seen_fields = std::collections::BTreeSet::new();
    for field in &extraction.fields {
        let (field_id, field_version) = match field {
            deepref_ai::ExtractedField::Value {
                field_id,
                field_version,
                ..
            }
            | deepref_ai::ExtractedField::InsufficientEvidence {
                field_id,
                field_version,
                ..
            } => (*field_id, *field_version),
        };
        if !seen_fields.insert((field_id, field_version)) {
            return Err(ExtractionError::InvalidValue(
                "each extraction field may appear only once".to_owned(),
            ));
        }
        let definition_row = sqlx::query(
            "SELECT definition.id,definition.project_id,definition.version,
                    definition.field_key,definition.label,definition.value_type,definition.required
             FROM extraction_field_definitions definition
             WHERE definition.project_id=$1 AND definition.id=$2 AND definition.version=$3",
        )
        .bind(project_id.as_uuid())
        .bind(field_id)
        .bind(i32::try_from(field_version).map_err(|_| {
            ExtractionError::InvalidValue("field definition version is too large".to_owned())
        })?)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(ExtractionError::DefinitionNotFound)?;
        let definition = field_from_row(definition_row)?;
        let latest_version: i32 = sqlx::query_scalar::<_, Option<i32>>(
            "SELECT max(version)
             FROM extraction_field_definitions
             WHERE project_id=$1 AND field_key=$2",
        )
        .bind(project_id.as_uuid())
        .bind(&definition.field_key)
        .fetch_one(&mut **tx)
        .await?
        .ok_or(ExtractionError::DefinitionNotFound)?;
        if latest_version
            != i32::try_from(definition.version).map_err(|_| {
                ExtractionError::InvalidValue("field definition version is too large".to_owned())
            })?
        {
            return Err(ExtractionError::StaleDefinitionVersion);
        }
        let deepref_ai::ExtractedField::Value {
            value,
            rationale,
            source,
            ..
        } = field
        else {
            // A person accepting a proposal must fill required fields; an
            // automatic apply (values flagged "to verify") just leaves them open.
            if definition.required && !options.needs_verification {
                return Err(ExtractionError::RequiredFieldInsufficient);
            }
            continue;
        };
        if options.skip_existing {
            let has_current: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM extraction_values
                 WHERE project_id=$1 AND study_id=$2 AND field_definition_id=$3
                   AND field_definition_version=$4 AND superseded_at IS NULL)",
            )
            .bind(project_id.as_uuid())
            .bind(study_id)
            .bind(definition.id)
            .bind(i32::try_from(definition.version).unwrap_or(i32::MAX))
            .fetch_one(&mut **tx)
            .await?;
            if has_current {
                outcome.skipped_existing += 1;
                continue;
            }
        }
        let (value_type, text_value, number_value, boolean_value, date_value) =
            database_value(value, &definition)?;
        let source_page = i32::try_from(source.page)
            .ok()
            .filter(|page| *page > 0)
            .ok_or_else(|| ExtractionError::InvalidValue("source page is invalid".to_owned()))?;
        if rationale.trim().is_empty() || rationale.len() > 4_000 {
            return Err(ExtractionError::InvalidValue(
                "rationale must contain 1 through 4000 characters".to_owned(),
            ));
        }
        let source_matches: bool = sqlx::query_scalar(
            "SELECT EXISTS(
               SELECT 1
               FROM study_reports sr
               JOIN documents d ON d.project_id=sr.project_id AND d.report_id=sr.report_id
                 AND d.id=$4
               JOIN document_blocks b ON b.document_id=d.id AND b.id=$5
               JOIN document_pages p ON p.document_id=d.id
                 AND p.parser_version=b.parser_version
                 AND p.page_number=b.page_number AND p.active
               WHERE sr.project_id=$1 AND sr.study_id=$2 AND sr.report_id=$3
                 AND d.active_parser_version=b.parser_version
                 AND b.active AND b.parser_version=$6
                 AND b.page_number=$7 AND b.content_hash=$8
             )",
        )
        .bind(project_id.as_uuid())
        .bind(study_id)
        .bind(source.report_id)
        .bind(source.document_id)
        .bind(source.document_block_id)
        .bind(&source.parser_version)
        .bind(source_page)
        .bind(&source.content_hash)
        .fetch_one(&mut **tx)
        .await?;
        if !source_matches
            || !deepref_ai::is_sha256(&source.content_hash)
            || source.parser_version.trim().is_empty()
        {
            return Err(ExtractionError::EvidenceNotInStudy);
        }
        prepared.push((
            rationale,
            source,
            definition,
            value_type,
            text_value,
            number_value,
            boolean_value,
            date_value,
            source_page,
        ));
    }
    let missing_required: bool = sqlx::query_scalar(
        "SELECT EXISTS(
           SELECT 1
           FROM extraction_field_definitions required_field
           WHERE required_field.project_id=$1
             AND required_field.required
             AND required_field.version = (
               SELECT max(version) FROM extraction_field_definitions latest
               WHERE latest.project_id=required_field.project_id
                 AND latest.field_key=required_field.field_key
             )
             AND NOT EXISTS(
               SELECT 1 FROM unnest($2::uuid[]) AS supplied(id)
               WHERE supplied.id=required_field.id
             )
         )",
    )
    .bind(project_id.as_uuid())
    .bind(seen_fields.iter().map(|(id, _)| *id).collect::<Vec<_>>())
    .fetch_one(&mut **tx)
    .await?;
    if missing_required {
        return Err(ExtractionError::RequiredFieldInsufficient);
    }

    let payload = serde_json::to_value(extraction)
        .map_err(|error| ExtractionError::InvalidValue(error.to_string()))?;
    for (
        rationale,
        source,
        definition,
        value_type,
        text_value,
        number_value,
        boolean_value,
        date_value,
        source_page,
    ) in prepared
    {
        let value_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO extraction_values
             (id,project_id,study_id,report_id,field_definition_id,field_definition_version,
              value_type,text_value,number_value,boolean_value,date_value,rationale,
              source_document_id,source_block_id,source_page,source_parser_version,
              source_content_hash,approved_by_actor_kind,approved_by_actor_id,needs_verification)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)",
        )
        .bind(value_id)
        .bind(project_id.as_uuid())
        .bind(study_id)
        .bind(source.report_id)
        .bind(definition.id)
        .bind(i32::try_from(definition.version).map_err(|_| {
            ExtractionError::InvalidValue("field definition version is too large".to_owned())
        })?)
        .bind(value_type)
        .bind(text_value)
        .bind(number_value)
        .bind(boolean_value)
        .bind(date_value)
        .bind(rationale)
        .bind(source.document_id)
        .bind(source.document_block_id)
        .bind(source_page)
        .bind(&source.parser_version)
        .bind(&source.content_hash)
        .bind(actor.kind().as_str())
        .bind(actor.id())
        .bind(options.needs_verification)
        .execute(&mut **tx)
        .await
        .map_err(|error| {
            if is_unique_violation(&error) {
                ExtractionError::ValueAlreadyApproved
            } else {
                ExtractionError::Database(error)
            }
        })?;
        outcome.inserted_value_ids.push(value_id);
    }
    sqlx::query(
        "INSERT INTO extraction_events
         (id,project_id,study_id,proposal_id,event_type,payload,actor_kind,actor_id)
         VALUES ($1,$2,$3,$4,'extraction_values_approved',$5,$6,$7)",
    )
    .bind(Uuid::new_v4())
    .bind(project_id.as_uuid())
    .bind(study_id)
    .bind(proposal_id)
    .bind(payload)
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .execute(&mut **tx)
    .await?;
    Ok(outcome)
}

/// A person confirms an AI-entered value; it stops being "to verify".
pub async fn confirm_value(
    pool: &PgPool,
    project_id: Uuid,
    study_id: Uuid,
    value_id: Uuid,
    actor: &Actor,
) -> Result<ExtractionValueRecord, ExtractionError> {
    let updated = sqlx::query(
        "UPDATE extraction_values
         SET needs_verification=false,verified_at=now(),
             verified_by_actor_kind=$4,verified_by_actor_id=$5
         WHERE project_id=$1 AND study_id=$2 AND id=$3 AND superseded_at IS NULL
         RETURNING id",
    )
    .bind(project_id)
    .bind(study_id)
    .bind(value_id)
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .fetch_optional(pool)
    .await?;
    if updated.is_none() {
        return Err(ExtractionError::StudyNotFound);
    }
    sqlx::query(
        "INSERT INTO review_events
         (id,project_id,event_type,aggregate_type,aggregate_id,payload,actor_kind,actor_id)
         VALUES ($1,$2,'extraction_value_verified','extraction_value',$3,$4,$5,$6)",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(value_id)
    .bind(serde_json::json!({"study_id": study_id}))
    .bind(actor.kind().as_str())
    .bind(actor.id())
    .execute(pool)
    .await?;
    list_values(pool, project_id, study_id)
        .await?
        .into_iter()
        .find(|value| value.id == value_id)
        .ok_or(ExtractionError::StudyNotFound)
}

fn field_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<ExtractionFieldDefinition, ExtractionError> {
    let value_type = row
        .try_get::<String, _>("value_type")
        .map_err(ExtractionError::Database)
        .and_then(|value| {
            ExtractionFieldType::parse(&value).ok_or_else(|| {
                ExtractionError::InvalidDefinition(format!("unknown value type {value}"))
            })
        })?;
    let version = u32::try_from(row.try_get::<i32, _>("version")?)
        .map_err(|_| ExtractionError::InvalidDefinition("version is negative".to_owned()))?;
    Ok(ExtractionFieldDefinition {
        id: row.get("id"),
        project_id: row.get::<Uuid, _>("project_id").into(),
        version,
        field_key: row.get("field_key"),
        label: row.get("label"),
        value_type,
        required: row.get("required"),
    })
}

fn value_from_row(row: sqlx::postgres::PgRow) -> Result<ExtractionValueRecord, ExtractionError> {
    let value_type: String = row.get("value_type");
    let value = match value_type.as_str() {
        "text" => ExtractionValue::Text {
            value: row.get::<String, _>("text_value"),
        },
        "number" => ExtractionValue::Number {
            value: row.get::<f64, _>("number_value"),
        },
        "boolean" => ExtractionValue::Boolean {
            value: row.get::<bool, _>("boolean_value"),
        },
        "date" => ExtractionValue::Date {
            value: row.get::<NaiveDate, _>("date_value"),
        },
        other => {
            return Err(ExtractionError::InvalidValue(format!(
                "unknown value type {other}"
            )));
        }
    };
    Ok(ExtractionValueRecord {
        id: row.get("id"),
        project_id: row.get::<Uuid, _>("project_id").into(),
        study_id: row.get("study_id"),
        report_id: row.get("report_id"),
        field_definition_id: row.get("field_definition_id"),
        field_definition_version: row.get("field_definition_version"),
        value,
        rationale: row.get("rationale"),
        source_document_id: row.get("source_document_id"),
        source_block_id: row.get("source_block_id"),
        source_page: row.get("source_page"),
        source_parser_version: row.get("source_parser_version"),
        source_content_hash: row.get("source_content_hash"),
        approved_by_actor_kind: row.get("approved_by_actor_kind"),
        approved_by_actor_id: row.get("approved_by_actor_id"),
        approved_at: row.get("approved_at"),
        needs_verification: row.get("needs_verification"),
        verified_at: row.get("verified_at"),
        verified_by: row.get("verified_by_actor_id"),
    })
}

fn database_value(
    value: &deepref_ai::TypedExtractionValue,
    definition: &ExtractionFieldDefinition,
) -> Result<DatabaseValue, ExtractionError> {
    let mismatch =
        || ExtractionError::InvalidValue("value type does not match field definition".to_owned());
    match (value, definition.value_type) {
        (deepref_ai::TypedExtractionValue::Text { value }, ExtractionFieldType::Text)
            if !value.trim().is_empty() =>
        {
            Ok(("text", Some(value.clone()), None, None, None))
        }
        (deepref_ai::TypedExtractionValue::Number { value }, ExtractionFieldType::Number)
            if value.is_finite() =>
        {
            Ok(("number", None, Some(*value), None, None))
        }
        (deepref_ai::TypedExtractionValue::Boolean { value }, ExtractionFieldType::Boolean) => {
            Ok(("boolean", None, None, Some(*value), None))
        }
        (deepref_ai::TypedExtractionValue::Date { value }, ExtractionFieldType::Date) => {
            let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map_err(|error| ExtractionError::InvalidValue(error.to_string()))?;
            Ok(("date", None, None, None, Some(date)))
        }
        _ => Err(mismatch()),
    }
}

fn extraction_validation_error(error: ExtractionValidationError) -> ExtractionError {
    ExtractionError::InvalidDefinition(error.to_string())
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(database) if database.code().as_deref() == Some("23505"))
}
