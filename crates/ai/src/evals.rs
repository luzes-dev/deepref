//! Pure, fixture-driven evaluation gates for consequential AI model changes.
//!
//! The evaluation format deliberately contains predictions rather than model
//! execution code. A reviewed gold set can therefore be compared repeatedly
//! and deterministically before a route is changed in production.

use std::{collections::BTreeSet, fmt};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::is_sha256;

pub const EVALUATION_SCHEMA: &str = "deepref.ai.evaluation";
pub const EVALUATION_VERSION: u32 = 1;
pub const MAX_DIAGNOSTICS: usize = 16;
const MAX_DIAGNOSTIC_LENGTH: usize = 240;
const MAX_CASES: usize = 10_000;
const MAX_CITATIONS_PER_CASE: usize = 256;
const MAX_TEXT_LENGTH: usize = 512;

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

    /// Validate the schema, reviewer/model metadata, case identity sets,
    /// finite numeric values, and closed prediction coverage.
    pub fn validate(&self) -> Result<(), EvaluationValidationError> {
        let mut diagnostics = Vec::new();
        if self.schema != EVALUATION_SCHEMA {
            add_validation(
                &mut diagnostics,
                format!("schema must be {EVALUATION_SCHEMA}"),
            );
        }
        if self.version != EVALUATION_VERSION {
            add_validation(
                &mut diagnostics,
                format!("version must be {EVALUATION_VERSION}"),
            );
        }
        validate_id(&mut diagnostics, "gold_set_id", &self.gold_set_id);
        validate_text(
            &mut diagnostics,
            "reviewer.reviewer_id",
            &self.reviewer.reviewer_id,
            MAX_TEXT_LENGTH,
        );
        validate_text(
            &mut diagnostics,
            "reviewer.review_notes",
            &self.reviewer.review_notes,
            4_000,
        );
        validate_thresholds(&mut diagnostics, &self.thresholds);

        if self.screening_cases.is_empty() {
            add_validation(
                &mut diagnostics,
                "screening_cases must not be empty".to_owned(),
            );
        }
        if self.numeric_extraction_cases.is_empty() {
            add_validation(
                &mut diagnostics,
                "numeric_extraction_cases must not be empty".to_owned(),
            );
        }
        if self.citation_cases.is_empty() {
            add_validation(
                &mut diagnostics,
                "citation_cases must not be empty".to_owned(),
            );
        }
        if self.screening_cases.len() > MAX_CASES
            || self.numeric_extraction_cases.len() > MAX_CASES
            || self.citation_cases.len() > MAX_CASES
        {
            add_validation(
                &mut diagnostics,
                "evaluation contains too many cases".to_owned(),
            );
        }

        let mut all_case_ids = BTreeSet::new();
        for case in &self.screening_cases {
            validate_case_id(&mut diagnostics, &mut all_case_ids, "screening", &case.id);
        }
        for case in &self.numeric_extraction_cases {
            validate_case_id(&mut diagnostics, &mut all_case_ids, "numeric", &case.id);
            validate_numeric_gold(&mut diagnostics, case);
        }
        for case in &self.citation_cases {
            validate_case_id(&mut diagnostics, &mut all_case_ids, "citation", &case.id);
            validate_evidence_list(
                &mut diagnostics,
                "citation gold evidence",
                &case.expected_evidence,
                true,
            );
        }

        let screening_ids = self
            .screening_cases
            .iter()
            .map(|case| case.id.clone())
            .collect::<BTreeSet<_>>();
        let numeric_ids = self
            .numeric_extraction_cases
            .iter()
            .map(|case| case.id.clone())
            .collect::<BTreeSet<_>>();
        let citation_ids = self
            .citation_cases
            .iter()
            .map(|case| case.id.clone())
            .collect::<BTreeSet<_>>();
        validate_model(
            &mut diagnostics,
            "baseline",
            &self.baseline,
            &screening_ids,
            &numeric_ids,
            &citation_ids,
        );
        validate_model(
            &mut diagnostics,
            "candidate",
            &self.candidate,
            &screening_ids,
            &numeric_ids,
            &citation_ids,
        );

        if !self
            .screening_cases
            .iter()
            .any(|case| matches!(case.truth, ScreeningTruth::Relevant))
        {
            add_validation(
                &mut diagnostics,
                "screening gold set needs a relevant case".to_owned(),
            );
        }
        if !self
            .screening_cases
            .iter()
            .any(|case| matches!(case.truth, ScreeningTruth::Irrelevant))
        {
            add_validation(
                &mut diagnostics,
                "screening gold set needs an irrelevant case".to_owned(),
            );
        }

        if diagnostics.is_empty() {
            Ok(())
        } else {
            Err(EvaluationValidationError { diagnostics })
        }
    }

    /// Compute baseline and candidate metrics over exactly the same reviewed
    /// cases. This function is pure after `validate` has parsed the boundary.
    pub fn evaluate(&self) -> Result<EvaluationReport, EvaluationError> {
        self.validate()?;
        Ok(EvaluationReport {
            baseline: evaluate_model(
                &self.baseline,
                &self.screening_cases,
                &self.numeric_extraction_cases,
                &self.citation_cases,
                &self.thresholds,
            )?,
            candidate: evaluate_model(
                &self.candidate,
                &self.screening_cases,
                &self.numeric_extraction_cases,
                &self.citation_cases,
                &self.thresholds,
            )?,
        })
    }

    /// Apply absolute quality thresholds and conservative comparison gates.
    /// A candidate may improve on baseline, but cannot silently trade away
    /// false exclusions or weighted safety loss beyond the checked-in policy.
    pub fn evaluate_and_gate(&self) -> Result<EvaluationReport, EvaluationGateError> {
        let report = self.evaluate()?;
        let mut diagnostics = Vec::new();
        let candidate = &report.candidate;
        let thresholds = &self.thresholds;

        gate_rate(
            &mut diagnostics,
            GateDiagnosticCode::SensitivityBelowThreshold,
            "sensitivity",
            candidate.screening.sensitivity,
            thresholds.min_sensitivity,
            |actual, minimum| actual < minimum,
        );
        gate_rate(
            &mut diagnostics,
            GateDiagnosticCode::SpecificityBelowThreshold,
            "specificity",
            candidate.screening.specificity,
            thresholds.min_specificity,
            |actual, minimum| actual < minimum,
        );
        gate_rate(
            &mut diagnostics,
            GateDiagnosticCode::FalseNegativeRateAboveThreshold,
            "false-negative rate",
            candidate.screening.false_negative_rate,
            thresholds.max_false_negative_rate,
            |actual, maximum| actual > maximum,
        );
        gate_rate(
            &mut diagnostics,
            GateDiagnosticCode::AbstentionRateAboveThreshold,
            "abstention rate",
            candidate.screening.abstention_rate,
            thresholds.max_abstention_rate,
            |actual, maximum| actual > maximum,
        );
        gate_rate(
            &mut diagnostics,
            GateDiagnosticCode::NumericAccuracyBelowThreshold,
            "numeric extraction accuracy",
            candidate.numeric.accuracy,
            thresholds.min_numeric_extraction_accuracy,
            |actual, minimum| actual < minimum,
        );
        gate_rate(
            &mut diagnostics,
            GateDiagnosticCode::CitationCorrectnessBelowThreshold,
            "evidence citation correctness",
            candidate.citations.accuracy,
            thresholds.min_citation_correctness,
            |actual, minimum| actual < minimum,
        );

        let allowed_false_negatives = report
            .baseline
            .screening
            .false_negatives
            .saturating_add(thresholds.max_false_exclusion_increase as usize);
        if candidate.screening.false_negatives > allowed_false_negatives {
            add_gate(
                &mut diagnostics,
                GateDiagnosticCode::FalseExclusionsRegressed,
                format!(
                    "false exclusions increased from {} to {} (allowed increase {})",
                    report.baseline.screening.false_negatives,
                    candidate.screening.false_negatives,
                    thresholds.max_false_exclusion_increase
                ),
            );
        }
        let allowed_loss =
            report.baseline.screening.weighted_loss + thresholds.max_weighted_loss_increase;
        if candidate.screening.weighted_loss > allowed_loss {
            add_gate(
                &mut diagnostics,
                GateDiagnosticCode::WeightedLossRegressed,
                format!(
                    "weighted screening loss increased from {:.6} to {:.6} (allowed increase {:.6})",
                    report.baseline.screening.weighted_loss,
                    candidate.screening.weighted_loss,
                    thresholds.max_weighted_loss_increase
                ),
            );
        }

        if diagnostics.is_empty() {
            Ok(report)
        } else {
            Err(EvaluationGateError::Rejected(GateRejected { diagnostics }))
        }
    }
}

fn evaluate_model(
    model: &ModelEvaluation,
    screening_cases: &[ScreeningCase],
    numeric_cases: &[NumericExtractionCase],
    citation_cases: &[CitationCase],
    thresholds: &EvaluationThresholds,
) -> Result<ModelMetrics, EvaluationError> {
    let mut relevant = 0;
    let mut irrelevant = 0;
    let mut true_positives = 0;
    let mut true_negatives = 0;
    let mut false_positives = 0;
    let mut false_negatives = 0;
    let mut abstentions = 0;

    for case in screening_cases {
        let prediction = model
            .screening_predictions
            .iter()
            .find(|prediction| prediction.case_id == case.id)
            .ok_or_else(|| missing_prediction("screening", &case.id))?;
        match (case.truth, prediction.prediction) {
            (ScreeningTruth::Relevant, ScreeningPrediction::Include) => {
                relevant += 1;
                true_positives += 1;
            }
            (ScreeningTruth::Relevant, ScreeningPrediction::Exclude) => {
                relevant += 1;
                false_negatives += 1;
            }
            (ScreeningTruth::Relevant, ScreeningPrediction::Maybe) => {
                relevant += 1;
                abstentions += 1;
            }
            (ScreeningTruth::Irrelevant, ScreeningPrediction::Include) => {
                irrelevant += 1;
                false_positives += 1;
            }
            (ScreeningTruth::Irrelevant, ScreeningPrediction::Exclude) => {
                irrelevant += 1;
                true_negatives += 1;
            }
            (ScreeningTruth::Irrelevant, ScreeningPrediction::Maybe) => {
                irrelevant += 1;
                abstentions += 1;
            }
        }
    }

    let total_screening = relevant + irrelevant;
    let screening = ScreeningMetrics {
        relevant,
        irrelevant,
        true_positives,
        true_negatives,
        false_positives,
        false_negatives,
        abstentions,
        sensitivity: ratio(true_positives, relevant),
        specificity: ratio(true_negatives, irrelevant),
        false_negative_rate: ratio(false_negatives, relevant),
        abstention_rate: ratio(abstentions, total_screening),
        weighted_loss: thresholds.false_exclusion_weight as f64 * false_negatives as f64
            + thresholds.unnecessary_inclusion_weight as f64 * false_positives as f64,
    };

    let mut numeric_correct = 0;
    let mut numeric_abstentions = 0;
    for case in numeric_cases {
        let prediction = model
            .numeric_predictions
            .iter()
            .find(|prediction| prediction.case_id == case.id)
            .ok_or_else(|| missing_prediction("numeric", &case.id))?;
        match prediction.prediction {
            NumericPrediction::Value { value }
                if numeric_matches(value, case.gold_value, case.tolerance) =>
            {
                numeric_correct += 1;
            }
            NumericPrediction::Abstain => numeric_abstentions += 1,
            NumericPrediction::Value { .. } => {}
        }
    }
    let numeric = NumericMetrics {
        total: numeric_cases.len(),
        correct: numeric_correct,
        abstentions: numeric_abstentions,
        accuracy: ratio(numeric_correct, numeric_cases.len()),
    };

    let mut citation_correct = 0;
    for case in citation_cases {
        let prediction = model
            .citation_predictions
            .iter()
            .find(|prediction| prediction.case_id == case.id)
            .ok_or_else(|| missing_prediction("citation", &case.id))?;
        let expected = case.expected_evidence.iter().collect::<BTreeSet<_>>();
        let actual = prediction.citations.iter().collect::<BTreeSet<_>>();
        if expected == actual {
            citation_correct += 1;
        }
    }
    let citations = CitationMetrics {
        total: citation_cases.len(),
        correct: citation_correct,
        accuracy: ratio(citation_correct, citation_cases.len()),
    };

    Ok(ModelMetrics {
        metadata: model.metadata.clone(),
        screening,
        numeric,
        citations,
    })
}

fn missing_prediction(family: &str, case_id: &EvaluationId) -> EvaluationError {
    EvaluationError::Invalid(EvaluationValidationError {
        diagnostics: vec![bounded_text(
            format!("{family} prediction is missing for {}", case_id.as_str()),
            MAX_DIAGNOSTIC_LENGTH,
        )],
    })
}

fn numeric_matches(predicted: f64, gold: f64, tolerance: NumericTolerance) -> bool {
    let difference = (predicted - gold).abs();
    tolerance
        .absolute
        .is_some_and(|absolute| difference <= absolute)
        || tolerance
            .relative
            .is_some_and(|relative| gold != 0.0 && difference / gold.abs() <= relative)
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    numerator as f64 / denominator as f64
}

fn validate_thresholds(diagnostics: &mut Vec<String>, thresholds: &EvaluationThresholds) {
    for (name, value) in [
        ("min_sensitivity", thresholds.min_sensitivity),
        ("min_specificity", thresholds.min_specificity),
        (
            "max_false_negative_rate",
            thresholds.max_false_negative_rate,
        ),
        ("max_abstention_rate", thresholds.max_abstention_rate),
        (
            "min_numeric_extraction_accuracy",
            thresholds.min_numeric_extraction_accuracy,
        ),
        (
            "min_citation_correctness",
            thresholds.min_citation_correctness,
        ),
    ] {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            add_validation(
                diagnostics,
                format!("{name} must be finite and within 0..=1"),
            );
        }
    }
    if thresholds.unnecessary_inclusion_weight == 0
        || thresholds.false_exclusion_weight
            < thresholds.unnecessary_inclusion_weight.saturating_mul(10)
    {
        add_validation(
            diagnostics,
            "false_exclusion_weight must be at least 10x a nonzero unnecessary_inclusion_weight"
                .to_owned(),
        );
    }
    if !thresholds.max_weighted_loss_increase.is_finite()
        || thresholds.max_weighted_loss_increase < 0.0
    {
        add_validation(
            diagnostics,
            "max_weighted_loss_increase must be finite and non-negative".to_owned(),
        );
    }
}

fn validate_model(
    diagnostics: &mut Vec<String>,
    model_name: &str,
    model: &ModelEvaluation,
    screening_ids: &BTreeSet<EvaluationId>,
    numeric_ids: &BTreeSet<EvaluationId>,
    citation_ids: &BTreeSet<EvaluationId>,
) {
    validate_text(
        diagnostics,
        &format!("{model_name}.metadata.provider"),
        &model.metadata.provider,
        MAX_TEXT_LENGTH,
    );
    validate_text(
        diagnostics,
        &format!("{model_name}.metadata.model"),
        &model.metadata.model,
        MAX_TEXT_LENGTH,
    );
    validate_text(
        diagnostics,
        &format!("{model_name}.metadata.model_version"),
        &model.metadata.model_version,
        MAX_TEXT_LENGTH,
    );
    if model.metadata.prompt_versions.is_empty() {
        add_validation(
            diagnostics,
            format!("{model_name}.metadata.prompt_versions must not be empty"),
        );
    }
    let mut prompt_tasks = BTreeSet::new();
    for prompt in &model.metadata.prompt_versions {
        validate_text(
            diagnostics,
            &format!("{model_name}.metadata.prompt.task"),
            &prompt.task,
            MAX_TEXT_LENGTH,
        );
        validate_text(
            diagnostics,
            &format!("{model_name}.metadata.prompt.version"),
            &prompt.version,
            MAX_TEXT_LENGTH,
        );
        if !prompt_tasks.insert(prompt.task.clone()) {
            add_validation(
                diagnostics,
                format!("{model_name} prompt tasks must be unique"),
            );
        }
    }

    let screening_prediction_ids = model
        .screening_predictions
        .iter()
        .map(|prediction| prediction.case_id.clone())
        .collect::<Vec<_>>();
    let numeric_prediction_ids = model
        .numeric_predictions
        .iter()
        .map(|prediction| prediction.case_id.clone())
        .collect::<Vec<_>>();
    let citation_prediction_ids = model
        .citation_predictions
        .iter()
        .map(|prediction| prediction.case_id.clone())
        .collect::<Vec<_>>();
    validate_prediction_ids(
        diagnostics,
        model_name,
        "screening",
        &screening_prediction_ids,
        screening_ids,
    );
    validate_prediction_ids(
        diagnostics,
        model_name,
        "numeric",
        &numeric_prediction_ids,
        numeric_ids,
    );
    validate_prediction_ids(
        diagnostics,
        model_name,
        "citation",
        &citation_prediction_ids,
        citation_ids,
    );

    for prediction in &model.numeric_predictions {
        if let NumericPrediction::Value { value } = prediction.prediction
            && !value.is_finite()
        {
            add_validation(
                diagnostics,
                format!("{model_name} numeric prediction must be finite"),
            );
        }
    }
    for prediction in &model.citation_predictions {
        if prediction.citations.len() > MAX_CITATIONS_PER_CASE {
            add_validation(
                diagnostics,
                format!("{model_name} citation prediction contains too many citations"),
            );
        }
        validate_evidence_list(
            diagnostics,
            &format!("{model_name} citation prediction"),
            &prediction.citations,
            false,
        );
    }
}

fn validate_prediction_ids(
    diagnostics: &mut Vec<String>,
    model_name: &str,
    family: &str,
    predicted: &[EvaluationId],
    expected: &BTreeSet<EvaluationId>,
) {
    let mut seen = BTreeSet::new();
    for id in predicted {
        validate_id(diagnostics, &format!("{model_name}.{family}.case_id"), id);
        if !seen.insert(id.clone()) {
            add_validation(
                diagnostics,
                format!("{model_name} {family} prediction IDs must be unique"),
            );
        }
        if !expected.contains(id) {
            add_validation(
                diagnostics,
                format!("{model_name} {family} prediction references an unknown case"),
            );
        }
    }
    for id in expected {
        if !seen.contains(id) {
            add_validation(
                diagnostics,
                format!("{model_name} {family} prediction is missing a gold case"),
            );
        }
    }
}

fn validate_numeric_gold(diagnostics: &mut Vec<String>, case: &NumericExtractionCase) {
    if !case.gold_value.is_finite() {
        add_validation(diagnostics, "numeric gold value must be finite".to_owned());
    }
    let tolerance = case.tolerance;
    if tolerance.absolute.is_none() && tolerance.relative.is_none() {
        add_validation(
            diagnostics,
            "numeric case needs an absolute and/or relative tolerance".to_owned(),
        );
    }
    for (name, value) in [
        ("absolute", tolerance.absolute),
        ("relative", tolerance.relative),
    ] {
        if value.is_some_and(|value| !value.is_finite() || value < 0.0) {
            add_validation(
                diagnostics,
                format!("numeric {name} tolerance must be finite and non-negative"),
            );
        }
    }
}

fn validate_evidence_list(
    diagnostics: &mut Vec<String>,
    name: &str,
    evidence: &[EvidenceIdentity],
    require_nonempty: bool,
) {
    if require_nonempty && evidence.is_empty() {
        add_validation(diagnostics, format!("{name} must not be empty"));
    }
    let mut seen = BTreeSet::new();
    for identity in evidence {
        validate_text(
            diagnostics,
            &format!("{name}.evidence_id"),
            &identity.evidence_id,
            MAX_TEXT_LENGTH,
        );
        if !is_sha256(&identity.content_hash) {
            add_validation(
                diagnostics,
                format!("{name} content hash must be lowercase SHA-256"),
            );
        }
        if !seen.insert(identity) {
            add_validation(
                diagnostics,
                format!("{name} identity/hash pairs must be unique"),
            );
        }
    }
}

fn validate_case_id(
    diagnostics: &mut Vec<String>,
    all_case_ids: &mut BTreeSet<EvaluationId>,
    family: &str,
    id: &EvaluationId,
) {
    validate_id(diagnostics, &format!("{family}.id"), id);
    if !all_case_ids.insert(id.clone()) {
        add_validation(
            diagnostics,
            "all evaluation case IDs must be unique".to_owned(),
        );
    }
}

fn validate_id(diagnostics: &mut Vec<String>, name: &str, id: &EvaluationId) {
    validate_text(diagnostics, name, id.as_str(), MAX_TEXT_LENGTH);
}

fn validate_text(diagnostics: &mut Vec<String>, name: &str, value: &str, max_length: usize) {
    if value.trim().is_empty() || value.chars().count() > max_length {
        add_validation(diagnostics, format!("{name} must be nonempty and bounded"));
    }
}

fn gate_rate<F: FnOnce(f64, f64) -> bool>(
    diagnostics: &mut Vec<GateDiagnostic>,
    code: GateDiagnosticCode,
    name: &str,
    actual: f64,
    threshold: f64,
    fails: F,
) {
    if fails(actual, threshold) {
        add_gate(
            diagnostics,
            code,
            format!(
                "{name} {:.6} does not satisfy configured threshold {:.6}",
                actual, threshold
            ),
        );
    }
}

fn add_validation(diagnostics: &mut Vec<String>, message: String) {
    if diagnostics.len() < MAX_DIAGNOSTICS {
        diagnostics.push(bounded_text(message, MAX_DIAGNOSTIC_LENGTH));
    }
}

fn add_gate(diagnostics: &mut Vec<GateDiagnostic>, code: GateDiagnosticCode, message: String) {
    if diagnostics.len() < MAX_DIAGNOSTICS {
        diagnostics.push(GateDiagnostic {
            code,
            message: bounded_text(message, MAX_DIAGNOSTIC_LENGTH),
        });
    }
}

fn bounded_text(value: String, max_length: usize) -> String {
    value.chars().take(max_length).collect()
}
