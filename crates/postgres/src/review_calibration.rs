//! Expert-adjudicated calibration evidence for automation-triggered reviews.
//!
//! A bundle authorizes automation for one screening stage, or for one standard
//! definition, against the exact compiled identity it was made for. Admission
//! reads the stored component snapshot and either admits the run or refuses it
//! with a typed reason that names what changed. Bundles are immutable: a changed
//! review needs a new bundle, never an edit to an old one.

use std::collections::BTreeSet;
use std::fmt;

use chrono::{DateTime, Utc};
use deepref_domain::ScreeningStage;
use deepref_review::{
    CalibrationBundleId, IdentityComparison, IdentityComponent, ReviewDefinitionKey,
    SemanticIdentity, worker::ReviewRunManifest,
};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use thiserror::Error;
use uuid::Uuid;

use crate::review_runs::PostgresReviewError;

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

/// A calibration bundle to record. `identity` is the compiled review it was
/// adjudicated for; obtain it from `preview_review_identity` or
/// `preview_screening_identity` rather than building it by hand.
#[derive(Debug, Clone)]
pub struct ReviewCalibrationBundleInput {
    pub id: CalibrationBundleId,
    pub project_id: Uuid,
    pub definition: ReviewDefinitionKey,
    pub identity: SemanticIdentity,
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
    #[error("review calibration identity could not be serialized")]
    Serialization(#[from] serde_json::Error),
}

/// Why an automation-triggered run may not use a calibration bundle. Each
/// refusal is a fail-closed outcome: no run is scheduled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalibrationRefusal {
    /// No bundle with this id exists for the project and definition.
    Missing,
    /// The bundle exists but its calibration did not pass.
    Failed,
    /// The bundle was made under another identity recipe, or its snapshot cannot
    /// be compared with the current review, so nothing about what changed is known.
    IncompatibleIdentityScheme { stored: u32, current: u32 },
    /// The bundle is for another screening stage, or for a non-screening review.
    StageMismatch {
        bundle: Option<ScreeningStage>,
        requested: Option<ScreeningStage>,
    },
    /// The bundle is for this review's stage, but these identity components have
    /// changed since it was made.
    Stale {
        components: BTreeSet<IdentityComponent>,
    },
}

impl CalibrationRefusal {
    /// The stable code for the HTTP API and the second-review gate record.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Missing => "calibration_missing",
            Self::Failed => "calibration_failed",
            Self::IncompatibleIdentityScheme { .. } => "calibration_incompatible",
            Self::StageMismatch { .. } => "calibration_stage_mismatch",
            Self::Stale { .. } => "calibration_stale",
        }
    }
}

impl fmt::Display for CalibrationRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => {
                formatter.write_str("automation-triggered review calibration is missing")
            }
            Self::Failed => {
                formatter.write_str("automation-triggered review calibration did not pass")
            }
            Self::IncompatibleIdentityScheme { stored: 1, .. } => formatter.write_str(
                "the review calibration predates stage-scoped identities (scheme 1) and \
                 cannot admit automation; calibrate again",
            ),
            Self::IncompatibleIdentityScheme { stored, current } if stored == current => {
                write!(
                    formatter,
                    "the review calibration (identity scheme {stored}) could not be compared \
                     with the current review; calibrate again"
                )
            }
            Self::IncompatibleIdentityScheme { stored, current } => write!(
                formatter,
                "the review calibration uses identity scheme {stored}, but the current review \
                 uses scheme {current}; calibrate again"
            ),
            Self::StageMismatch { bundle, requested } => write!(
                formatter,
                "the review calibration is for {}, not for {}",
                describe_stage(*bundle),
                describe_stage(*requested)
            ),
            Self::Stale { components } if components.is_empty() => formatter.write_str(
                "the review calibration no longer matches the compiled review; calibrate again",
            ),
            Self::Stale { components } => {
                let changed = components
                    .iter()
                    .map(|component| component.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    formatter,
                    "the review calibration no longer matches the compiled review (changed: \
                     {changed}); calibrate again"
                )
            }
        }
    }
}

impl std::error::Error for CalibrationRefusal {}

fn describe_stage(stage: Option<ScreeningStage>) -> &'static str {
    match stage {
        Some(ScreeningStage::TitleAbstract) => "title/abstract screening",
        Some(ScreeningStage::FullText) => "full-text screening",
        None => "a non-screening review",
    }
}

/// The calibration fields that admission reads, as stored.
#[derive(Debug, Clone)]
pub(crate) struct StoredCalibration {
    pub(crate) status: String,
    pub(crate) stage: Option<String>,
    pub(crate) identity_scheme: i32,
    pub(crate) semantic_bundle_hash: String,
    pub(crate) identity_snapshot: Option<Value>,
}

/// The stage vocabulary of the `stage` column and of the second-review gate.
pub(crate) const fn stage_key(stage: ScreeningStage) -> &'static str {
    match stage {
        ScreeningStage::TitleAbstract => "title_abstract",
        ScreeningStage::FullText => "full_text",
    }
}

fn stage_from_key(value: &str) -> Option<ScreeningStage> {
    match value {
        "title_abstract" => Some(ScreeningStage::TitleAbstract),
        "full_text" => Some(ScreeningStage::FullText),
        _ => None,
    }
}

pub async fn insert_review_calibration_bundle(
    pool: &PgPool,
    input: ReviewCalibrationBundleInput,
) -> Result<(), ReviewCalibrationError> {
    validate_input(&input)?;
    let semantic_bundle_hash = input
        .identity
        .aggregate_hash()
        .map_err(|error| ReviewCalibrationError::InvalidInput(error.to_string()))?;
    let identity_scheme = i32::try_from(input.identity.scheme).map_err(|_| {
        ReviewCalibrationError::InvalidInput("identity scheme is out of range".to_owned())
    })?;
    let identity_snapshot = serde_json::to_value(&input.identity)?;
    let stage = input.identity.stage.map(stage_key);
    let eval_set = input.evaluation_set_id.trim();
    sqlx::query(
        "INSERT INTO review_calibration_bundles
         (id,project_id,definition_key,stage,identity_scheme,identity_snapshot,
          semantic_bundle_hash,evaluation_set_id,thresholds,metrics,reviewer_metadata,
          status,evaluated_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
    )
    .bind(input.id.as_uuid())
    .bind(input.project_id)
    .bind(input.definition.as_str())
    .bind(stage)
    .bind(identity_scheme)
    .bind(identity_snapshot)
    .bind(semantic_bundle_hash.as_str())
    .bind(eval_set)
    .bind(input.thresholds)
    .bind(input.metrics)
    .bind(input.reviewer_metadata)
    .bind(input.status.as_str())
    .bind(input.evaluated_at)
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
    // `validate` also requires a stage exactly for screening and the full
    // component set of the current recipe.
    input
        .identity
        .validate()
        .map_err(|error| ReviewCalibrationError::InvalidInput(error.to_string()))?;
    if input.identity.definition != input.definition {
        return Err(ReviewCalibrationError::InvalidInput(
            "calibration identity is for another review definition".to_owned(),
        ));
    }
    Ok(())
}

/// Admits an automation-triggered run against one calibration bundle, inside
/// the transaction that would schedule it.
pub(crate) async fn admit_calibration(
    transaction: &mut Transaction<'_, Postgres>,
    manifest: &ReviewRunManifest,
    calibration_bundle_id: CalibrationBundleId,
) -> Result<(), PostgresReviewError> {
    let row = sqlx::query(
        "SELECT status,stage,identity_scheme,semantic_bundle_hash,identity_snapshot
         FROM review_calibration_bundles
         WHERE project_id=$1 AND id=$2 AND definition_key=$3",
    )
    .bind(manifest.project_id.as_uuid())
    .bind(calibration_bundle_id.as_uuid())
    .bind(manifest.definition.as_str())
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(row) = row else {
        return Err(CalibrationRefusal::Missing.into());
    };
    let stored = StoredCalibration {
        status: row.get("status"),
        stage: row.get("stage"),
        identity_scheme: row.get("identity_scheme"),
        semantic_bundle_hash: row.get("semantic_bundle_hash"),
        identity_snapshot: row.get("identity_snapshot"),
    };
    check_admission(&stored, manifest).map_err(PostgresReviewError::from)
}

/// Decides admission from a stored calibration row and the compiled manifest.
///
/// A specific refusal is given only when the stored snapshot supports it. A
/// snapshot that is missing, unreadable, or does not reproduce the stored hash
/// is reported as an incompatible identity, because nothing about what changed
/// can be trusted.
pub(crate) fn check_admission(
    stored: &StoredCalibration,
    manifest: &ReviewRunManifest,
) -> Result<(), CalibrationRefusal> {
    if stored.status != ReviewCalibrationStatus::Passing.as_str() {
        return Err(CalibrationRefusal::Failed);
    }
    let current_scheme = manifest.identity_scheme();
    let stored_scheme = u32::try_from(stored.identity_scheme).unwrap_or(0);
    let incompatible = || CalibrationRefusal::IncompatibleIdentityScheme {
        stored: stored_scheme,
        current: current_scheme,
    };
    if stored_scheme != current_scheme || stored_scheme < 2 {
        return Err(incompatible());
    }
    let Some(current) = manifest.semantic_identity.as_ref() else {
        return Err(incompatible());
    };
    let stored_stage = match stored.stage.as_deref() {
        None => None,
        Some(value) => Some(stage_from_key(value).ok_or_else(incompatible)?),
    };
    if stored_stage != current.stage {
        return Err(CalibrationRefusal::StageMismatch {
            bundle: stored_stage,
            requested: current.stage,
        });
    }
    if stored.semantic_bundle_hash.as_str() == manifest.semantic_bundle_hash.as_str() {
        return Ok(());
    }
    let snapshot = stored
        .identity_snapshot
        .clone()
        .and_then(|value| serde_json::from_value::<SemanticIdentity>(value).ok())
        .ok_or_else(incompatible)?;
    let reproduces_stored_hash = snapshot
        .aggregate_hash()
        .is_ok_and(|hash| hash.as_str() == stored.semantic_bundle_hash.as_str());
    if !reproduces_stored_hash {
        return Err(incompatible());
    }
    match snapshot.compare(current) {
        IdentityComparison::Stale(components) => Err(CalibrationRefusal::Stale { components }),
        IdentityComparison::StageMismatch {
            stored: bundle,
            current: requested,
        } => Err(CalibrationRefusal::StageMismatch { bundle, requested }),
        IdentityComparison::IncompatibleScheme {
            stored: stored_identity_scheme,
            current: current_identity_scheme,
        } => Err(CalibrationRefusal::IncompatibleIdentityScheme {
            stored: stored_identity_scheme,
            current: current_identity_scheme,
        }),
        // Same components with a different aggregate, or another definition, is
        // inconsistent evidence rather than a known change.
        IdentityComparison::Same | IdentityComparison::DefinitionMismatch { .. } => {
            Err(incompatible())
        }
    }
}

#[cfg(test)]
mod tests {
    use deepref_ai::ModelProfile;
    use deepref_domain::{ProjectId, ProtocolVersionId, ReportId};
    use deepref_review::{
        ReviewOrigin, ReviewSubject,
        worker::{
            CompiledReview, ReviewHash, ReviewManifestInput, ReviewModelIdentity,
            ReviewRuntimeIdentity,
        },
    };
    use uuid::Uuid;

    use super::*;

    fn hash(value: &str) -> ReviewHash {
        ReviewHash::digest_bytes(value)
    }

    fn screening_manifest(stage: ScreeningStage, model_version: &str) -> ReviewRunManifest {
        let profile = match stage {
            ScreeningStage::TitleAbstract => ModelProfile::Reasoning,
            ScreeningStage::FullText => ModelProfile::LongContextReasoning,
        };
        CompiledReview::compile(ReviewDefinitionKey::Screening)
            .expect("screening definition compiles")
            .build_manifest(ReviewManifestInput {
                project_id: ProjectId::new(Uuid::new_v4()),
                subject: ReviewSubject::Screening {
                    report_id: ReportId::new(Uuid::new_v4()),
                    stage,
                    protocol_version_id: ProtocolVersionId::new(Uuid::new_v4()),
                    expected_revision: 0,
                },
                origin: ReviewOrigin::ReviewerRequested,
                protocol_version_id: None,
                protocol_hash: hash("protocol"),
                source_manifest_hash: hash("source-manifest"),
                source_content_hash: hash("source"),
                resolved_models: vec![ReviewModelIdentity {
                    profile,
                    provider: "fixture".to_owned(),
                    model: "reasoner".to_owned(),
                    model_version: model_version.to_owned(),
                    parameters_hash: hash("parameters"),
                    endpoint: None,
                }],
                runtime: ReviewRuntimeIdentity {
                    build_sha: hash("build"),
                    rust_version: "1.95".to_owned(),
                    target: "test".to_owned(),
                    deployment_build_id: None,
                },
            })
            .expect("screening manifest builds")
    }

    /// The row a bundle recorded for `identity` would hold.
    fn stored_for(identity: &SemanticIdentity) -> StoredCalibration {
        StoredCalibration {
            status: ReviewCalibrationStatus::Passing.as_str().to_owned(),
            stage: identity.stage.map(|stage| stage_key(stage).to_owned()),
            identity_scheme: i32::try_from(identity.scheme).expect("scheme fits"),
            semantic_bundle_hash: identity
                .aggregate_hash()
                .expect("identity hashes")
                .as_str()
                .to_owned(),
            identity_snapshot: Some(serde_json::to_value(identity).expect("identity serializes")),
        }
    }

    fn with_component(
        identity: &SemanticIdentity,
        component: IdentityComponent,
        value: &str,
    ) -> SemanticIdentity {
        let mut changed = identity.clone();
        changed.components.insert(component, hash(value));
        changed
    }

    #[test]
    fn an_exact_identity_admits_through_the_fast_path() {
        let manifest = screening_manifest(ScreeningStage::TitleAbstract, "v1");
        let identity = manifest.semantic_identity.clone().expect("identity");
        assert_eq!(check_admission(&stored_for(&identity), &manifest), Ok(()));
    }

    #[test]
    fn an_unknown_scheme_cannot_use_the_matching_hash_fast_path() {
        let manifest = screening_manifest(ScreeningStage::TitleAbstract, "v1");
        let identity = manifest.semantic_identity.clone().expect("identity");
        let mut stored = stored_for(&identity);
        stored.identity_scheme = 3;
        assert_eq!(
            check_admission(&stored, &manifest),
            Err(CalibrationRefusal::IncompatibleIdentityScheme {
                stored: 3,
                current: 2
            }),
        );
    }

    #[test]
    fn a_failed_calibration_is_refused_before_its_identity_is_read() {
        let manifest = screening_manifest(ScreeningStage::TitleAbstract, "v1");
        let identity = manifest.semantic_identity.clone().expect("identity");
        let mut stored = stored_for(&identity);
        stored.status = ReviewCalibrationStatus::Failed.as_str().to_owned();
        stored.identity_scheme = 1;
        assert_eq!(
            check_admission(&stored, &manifest),
            Err(CalibrationRefusal::Failed)
        );
    }

    #[test]
    fn a_legacy_scheme_one_bundle_never_admits_even_with_a_matching_hash() {
        let manifest = screening_manifest(ScreeningStage::TitleAbstract, "v1");
        let identity = manifest.semantic_identity.clone().expect("identity");
        let mut stored = stored_for(&identity);
        stored.identity_scheme = 1;
        stored.stage = None;
        stored.identity_snapshot = None;
        assert_eq!(
            check_admission(&stored, &manifest),
            Err(CalibrationRefusal::IncompatibleIdentityScheme {
                stored: 1,
                current: 2,
            })
        );
    }

    #[test]
    fn a_bundle_for_the_other_stage_is_a_stage_mismatch() {
        let title = screening_manifest(ScreeningStage::TitleAbstract, "v1");
        let full = screening_manifest(ScreeningStage::FullText, "v1");
        let stored = stored_for(&full.semantic_identity.clone().expect("identity"));
        assert_eq!(
            check_admission(&stored, &title),
            Err(CalibrationRefusal::StageMismatch {
                bundle: Some(ScreeningStage::FullText),
                requested: Some(ScreeningStage::TitleAbstract),
            })
        );
    }

    #[test]
    fn a_changed_component_is_named_as_stale() {
        let manifest = screening_manifest(ScreeningStage::FullText, "v1");
        let identity = manifest.semantic_identity.clone().expect("identity");
        let older = with_component(&identity, IdentityComponent::Models, "older route");
        assert_eq!(
            check_admission(&stored_for(&older), &manifest),
            Err(CalibrationRefusal::Stale {
                components: BTreeSet::from([IdentityComponent::Models]),
            })
        );
    }

    #[test]
    fn a_component_set_change_is_incompatible_not_stale() {
        let manifest = screening_manifest(ScreeningStage::TitleAbstract, "v1");
        let identity = manifest.semantic_identity.clone().expect("identity");
        let mut narrower = identity.clone();
        narrower.components.remove(&IdentityComponent::Policy);
        assert!(matches!(
            check_admission(&stored_for(&narrower), &manifest),
            Err(CalibrationRefusal::IncompatibleIdentityScheme { .. })
        ));
    }

    #[test]
    fn a_snapshot_that_does_not_reproduce_its_hash_is_incompatible() {
        let manifest = screening_manifest(ScreeningStage::TitleAbstract, "v1");
        let identity = manifest.semantic_identity.clone().expect("identity");
        let mut stored = stored_for(&with_component(
            &identity,
            IdentityComponent::Models,
            "older",
        ));
        stored.semantic_bundle_hash = "c".repeat(64);
        assert!(matches!(
            check_admission(&stored, &manifest),
            Err(CalibrationRefusal::IncompatibleIdentityScheme { .. })
        ));
    }

    #[test]
    fn an_identical_snapshot_with_another_hash_is_inconsistent_not_stale() {
        let manifest = screening_manifest(ScreeningStage::TitleAbstract, "v1");
        let identity = manifest.semantic_identity.clone().expect("identity");
        let mut stored = stored_for(&identity);
        stored.semantic_bundle_hash = "d".repeat(64);
        assert!(matches!(
            check_admission(&stored, &manifest),
            Err(CalibrationRefusal::IncompatibleIdentityScheme {
                stored: 2,
                current: 2
            })
        ));
    }

    #[test]
    fn an_unreadable_snapshot_is_incompatible() {
        let manifest = screening_manifest(ScreeningStage::TitleAbstract, "v1");
        let identity = manifest.semantic_identity.clone().expect("identity");
        let mut stored = stored_for(&with_component(
            &identity,
            IdentityComponent::Models,
            "older",
        ));
        stored.identity_snapshot = Some(serde_json::json!({"scheme": 2}));
        assert!(matches!(
            check_admission(&stored, &manifest),
            Err(CalibrationRefusal::IncompatibleIdentityScheme { .. })
        ));
    }

    #[test]
    fn refusals_name_the_stage_and_the_changed_components() {
        let stale = CalibrationRefusal::Stale {
            components: BTreeSet::from([IdentityComponent::Protocol, IdentityComponent::Models]),
        };
        assert_eq!(stale.code(), "calibration_stale");
        assert_eq!(
            stale.to_string(),
            "the review calibration no longer matches the compiled review (changed: protocol, \
             models); calibrate again"
        );
        let mismatch = CalibrationRefusal::StageMismatch {
            bundle: Some(ScreeningStage::FullText),
            requested: Some(ScreeningStage::TitleAbstract),
        };
        assert_eq!(mismatch.code(), "calibration_stage_mismatch");
        assert_eq!(
            mismatch.to_string(),
            "the review calibration is for full-text screening, not for title/abstract screening"
        );
        assert_eq!(CalibrationRefusal::Missing.code(), "calibration_missing");
        assert_eq!(CalibrationRefusal::Failed.code(), "calibration_failed");
    }
}
