//! Pure, fixture-driven evaluation gates for consequential AI model changes.
//!
//! The evaluation format deliberately contains predictions rather than model
//! execution code. A reviewed gold set can therefore be compared repeatedly
//! and deterministically before a route is changed in production.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const EVALUATION_SCHEMA: &str = "deepref.ai.evaluation";
pub const EVALUATION_VERSION: u32 = 1;
pub const MAX_DIAGNOSTICS: usize = 16;
pub(super) const MAX_DIAGNOSTIC_LENGTH: usize = 240;
pub(super) const MAX_CASES: usize = 10_000;
pub(super) const MAX_CITATIONS_PER_CASE: usize = 256;
pub(super) const MAX_TEXT_LENGTH: usize = 512;

/// An identifier in a reviewed evaluation set. It is intentionally distinct
/// from domain identifiers: evaluation cases are methodological fixtures, not
/// records that can be acted on by the application.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EvaluationId(String);

impl EvaluationId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Metadata identifying the reviewer and the review event that produced a
/// gold set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewerMetadata {
    pub reviewer_id: String,
    pub reviewed_at: DateTime<Utc>,
    pub review_notes: String,
}

/// One prompt version per task family used to produce a model's predictions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptVersion {
    pub task: String,
    pub version: String,
}

/// Provider/model identity captured alongside predictions so a gate cannot be
/// mistaken for an evaluation of an anonymous or silently changed route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelMetadata {
    pub provider: String,
    pub model: String,
    pub model_version: String,
    pub prompt_versions: Vec<PromptVersion>,
}

/// The reviewed truth label for a screening case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreeningTruth {
    Relevant,
    Irrelevant,
}

/// The only screening predictions a candidate may make. `Maybe` is retained
/// for review and is therefore not a false negative, but it is an abstention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreeningPrediction {
    Include,
    Exclude,
    Maybe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreeningCase {
    pub id: EvaluationId,
    pub truth: ScreeningTruth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreeningPredictionCase {
    pub case_id: EvaluationId,
    pub prediction: ScreeningPrediction,
}

/// Gold tolerance for a numeric extraction. At least one finite,
/// non-negative tolerance is required. A prediction passes when its absolute
/// error is within `absolute` OR its relative error is within `relative`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumericTolerance {
    pub absolute: Option<f64>,
    pub relative: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumericExtractionCase {
    pub id: EvaluationId,
    pub gold_value: f64,
    pub tolerance: NumericTolerance,
}

/// Numeric values are numbers at the JSON boundary; there is deliberately no
/// string-to-number fallback that could hide parsing or locale errors.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum NumericPrediction {
    Value { value: f64 },
    Abstain,
}

/// Durable evidence identity is the pair, not article text. Citation scoring
/// compares exact identity/hash pairs and never searches or scores text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceIdentity {
    pub evidence_id: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CitationCase {
    pub id: EvaluationId,
    pub expected_evidence: Vec<EvidenceIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CitationPredictionCase {
    pub case_id: EvaluationId,
    pub citations: Vec<EvidenceIdentity>,
}

/// Explicit gate policy stored with the gold set. Rates are in `[0, 1]`.
/// `false_exclusion_weight` must be at least ten times the cost of an
/// unnecessary inclusion.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationThresholds {
    pub min_sensitivity: f64,
    pub min_specificity: f64,
    pub max_false_negative_rate: f64,
    pub max_abstention_rate: f64,
    pub min_numeric_extraction_accuracy: f64,
    pub min_citation_correctness: f64,
    pub false_exclusion_weight: u32,
    pub unnecessary_inclusion_weight: u32,
    pub max_false_exclusion_increase: u32,
    pub max_weighted_loss_increase: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelEvaluation {
    pub metadata: ModelMetadata,
    pub screening_predictions: Vec<ScreeningPredictionCase>,
    pub numeric_predictions: Vec<NumericPredictionCase>,
    pub citation_predictions: Vec<CitationPredictionCase>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumericPredictionCase {
    pub case_id: EvaluationId,
    pub prediction: NumericPrediction,
}

/// One manually reviewed, versioned gold set and two prediction sets over the
/// same cases. Baseline and candidate are evaluated with identical semantics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationSet {
    pub schema: String,
    pub version: u32,
    pub gold_set_id: EvaluationId,
    pub reviewer: ReviewerMetadata,
    pub thresholds: EvaluationThresholds,
    pub screening_cases: Vec<ScreeningCase>,
    pub numeric_extraction_cases: Vec<NumericExtractionCase>,
    pub citation_cases: Vec<CitationCase>,
    pub baseline: ModelEvaluation,
    pub candidate: ModelEvaluation,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("evaluation input is invalid")]
pub struct EvaluationValidationError {
    diagnostics: Vec<String>,
}

impl EvaluationValidationError {
    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EvaluationError {
    #[error("evaluation JSON is invalid: {message}")]
    Json { message: String },
    #[error(transparent)]
    Invalid(#[from] EvaluationValidationError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GateDiagnosticCode {
    SensitivityBelowThreshold,
    SpecificityBelowThreshold,
    FalseNegativeRateAboveThreshold,
    AbstentionRateAboveThreshold,
    NumericAccuracyBelowThreshold,
    CitationCorrectnessBelowThreshold,
    FalseExclusionsRegressed,
    WeightedLossRegressed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GateDiagnostic {
    pub code: GateDiagnosticCode,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateRejected {
    diagnostics: Vec<GateDiagnostic>,
}

impl GateRejected {
    pub fn diagnostics(&self) -> &[GateDiagnostic] {
        &self.diagnostics
    }
}

impl fmt::Display for GateRejected {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "candidate failed evaluation gate")
    }
}

impl std::error::Error for GateRejected {}

#[derive(Debug, Clone, PartialEq)]
pub enum EvaluationGateError {
    Evaluation(EvaluationError),
    Rejected(GateRejected),
}

impl fmt::Display for EvaluationGateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Evaluation(error) => error.fmt(formatter),
            Self::Rejected(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for EvaluationGateError {}

impl From<EvaluationError> for EvaluationGateError {
    fn from(error: EvaluationError) -> Self {
        Self::Evaluation(error)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScreeningMetrics {
    pub relevant: usize,
    pub irrelevant: usize,
    pub true_positives: usize,
    pub true_negatives: usize,
    pub false_positives: usize,
    pub false_negatives: usize,
    pub abstentions: usize,
    pub sensitivity: f64,
    pub specificity: f64,
    pub false_negative_rate: f64,
    pub abstention_rate: f64,
    pub weighted_loss: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NumericMetrics {
    pub total: usize,
    pub correct: usize,
    pub abstentions: usize,
    pub accuracy: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CitationMetrics {
    pub total: usize,
    pub correct: usize,
    pub accuracy: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelMetrics {
    pub metadata: ModelMetadata,
    pub screening: ScreeningMetrics,
    pub numeric: NumericMetrics,
    pub citations: CitationMetrics,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvaluationReport {
    pub baseline: ModelMetrics,
    pub candidate: ModelMetrics,
}

mod gate;
mod metrics;
mod validation;

impl EvaluationSet {
    /// Parse and validate a checked-in fixture at the JSON boundary.
    pub fn from_json(input: &str) -> Result<Self, EvaluationError> {
        let evaluation: Self =
            serde_json::from_str(input).map_err(|error| EvaluationError::Json {
                message: bounded_text(error.to_string(), MAX_DIAGNOSTIC_LENGTH),
            })?;
        evaluation.validate()?;
        Ok(evaluation)
    }

    /// Validate schema, metadata, identities, values, and prediction coverage.
    pub fn validate(&self) -> Result<(), EvaluationValidationError> {
        validation::validate(self)
    }

    /// Compute baseline and candidate metrics over the same reviewed cases.
    pub fn evaluate(&self) -> Result<EvaluationReport, EvaluationError> {
        metrics::evaluate(self)
    }

    /// Apply absolute quality thresholds and conservative comparison gates.
    pub fn evaluate_and_gate(&self) -> Result<EvaluationReport, EvaluationGateError> {
        gate::evaluate(self)
    }
}

fn bounded_text(value: String, max_length: usize) -> String {
    value.chars().take(max_length).collect()
}
