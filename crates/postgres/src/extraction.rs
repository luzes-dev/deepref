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
    pub report_id: Uuid,
    pub field_definition_id: Uuid,
    pub field_definition_version: i32,
    pub value: ExtractionValue,
    pub rationale: String,
    pub source_document_id: Uuid,
    pub source_block_id: Uuid,
    pub source_page: i32,
    pub source_parser_version: String,
    pub source_content_hash: String,
    pub approved_by_actor_kind: String,
    pub approved_by_actor_id: String,
    pub approved_at: DateTime<Utc>,
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
}

pub async fn list_field_definitions(
    pool: &PgPool,
    project_id: Uuid,
) -> Result<Vec<ExtractionFieldDefinition>, ExtractionError> {
    let rows = sqlx::query(
        "SELECT DISTINCT ON (field_key)
                id, project_id, version, field_key, label, value_type, required
         FROM extraction_field_definitions
         WHERE project_id=$1
         ORDER BY field_key, version DESC, id",
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
                approved_by_actor_id,approved_at
         FROM extraction_values
         WHERE project_id=$1 AND study_id=$2 ORDER BY field_definition_id,field_definition_version,id",
    )
    .bind(project_id)
    .bind(study_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(value_from_row).collect()
}

pub async fn apply_data_extraction_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    project_id: ProjectId,
    study_id: Uuid,
    proposal_id: Uuid,
    extraction: &deepref_ai::DataExtraction,
    actor: &Actor,
) -> Result<(), ExtractionError> {
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
            if definition.required {
                return Err(ExtractionError::RequiredFieldInsufficient);
            }
            continue;
        };
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
        sqlx::query(
            "INSERT INTO extraction_values
             (id,project_id,study_id,report_id,field_definition_id,field_definition_version,
              value_type,text_value,number_value,boolean_value,date_value,rationale,
              source_document_id,source_block_id,source_page,source_parser_version,
              source_content_hash,approved_by_actor_kind,approved_by_actor_id)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)",
        )
        .bind(Uuid::new_v4())
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
        .execute(&mut **tx)
        .await
        .map_err(|error| {
            if is_unique_violation(&error) {
                ExtractionError::ValueAlreadyApproved
            } else {
                ExtractionError::Database(error)
            }
        })?;
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
    Ok(())
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
