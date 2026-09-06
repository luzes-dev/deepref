use chrono::{DateTime, Utc};
use deepref_review::{
    CalibrationBundleId, ReviewDefinitionKey,
    worker::{ReviewHash, ReviewRunManifest},
};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewCalibrationStatus {
    Passing,
    Failed,
}

impl ReviewCalibrationStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Passing => "passing",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReviewCalibrationBundleInput {
    pub id: CalibrationBundleId,
    pub project_id: Uuid,
    pub definition: ReviewDefinitionKey,
    pub semantic_bundle_hash: ReviewHash,
    pub evaluation_set_id: String,
    pub thresholds: Value,
    pub metrics: Value,
    pub reviewer_metadata: Value,
    pub status: ReviewCalibrationStatus,
    pub evaluated_at: DateTime<Utc>,
}

#[derive(Debug, Error)]
pub enum ReviewCalibrationError {
    #[error("review calibration database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("review calibration input is invalid: {0}")]
    InvalidInput(String),
}

pub async fn insert_review_calibration_bundle(
    pool: &PgPool,
    input: ReviewCalibrationBundleInput,
) -> Result<(), ReviewCalibrationError> {
    validate_input(&input)?;
    let id = input.id.as_uuid();
    let def = input.definition.as_str();
    let hash = input.semantic_bundle_hash.as_str();
    let eval_set = input.evaluation_set_id.trim();
    let status = input.status.as_str();
    sqlx::query!(
        r#"INSERT INTO review_calibration_bundles
         (id,project_id,definition_key,semantic_bundle_hash,evaluation_set_id,
          thresholds,metrics,reviewer_metadata,status,evaluated_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)"#,
        id,
        input.project_id,
        def,
        hash,
        eval_set,
        input.thresholds,
        input.metrics,
        input.reviewer_metadata,
        status,
        input.evaluated_at,
    )
    .execute(pool)
    .await?;
    Ok(())
}

fn validate_input(input: &ReviewCalibrationBundleInput) -> Result<(), ReviewCalibrationError> {
    let evaluation_set_id = input.evaluation_set_id.trim();
    if input.project_id.is_nil()
        || evaluation_set_id.is_empty()
        || evaluation_set_id.chars().count() > 500
        || !input.thresholds.is_object()
        || !input.metrics.is_object()
        || !input.reviewer_metadata.is_object()
    {
        return Err(ReviewCalibrationError::InvalidInput(
            "project, evaluation set, thresholds, metrics, and reviewer metadata are required"
                .to_owned(),
        ));
    }
    Ok(())
}

#[derive(Debug, Error)]
pub(crate) enum CalibrationAdmissionError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("review calibration is missing")]
    Missing,
    #[error("review calibration failed")]
    Failed,
    #[error("review calibration is stale")]
    Stale,
}

pub(crate) async fn admit_calibration(
    transaction: &mut Transaction<'_, Postgres>,
    manifest: &ReviewRunManifest,
    calibration_bundle_id: CalibrationBundleId,
) -> Result<(), CalibrationAdmissionError> {
    let project_id = manifest.project_id.as_uuid();
    let cal_id = calibration_bundle_id.as_uuid();
    let def = manifest.definition.as_str();
    let row = sqlx::query!(
        r#"SELECT semantic_bundle_hash, status
         FROM review_calibration_bundles
         WHERE project_id = $1 AND id = $2 AND definition_key = $3"#,
        project_id,
        cal_id,
        def,
    )
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(row) = row else {
        return Err(CalibrationAdmissionError::Missing);
    };
    if row.status != "passing" {
        return Err(CalibrationAdmissionError::Failed);
    }
    if row.semantic_bundle_hash != manifest.semantic_bundle_hash.as_str() {
        return Err(CalibrationAdmissionError::Stale);
    }
    Ok(())
}
