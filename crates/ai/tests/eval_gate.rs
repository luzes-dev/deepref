use deepref_ai::{
    EvaluationGateError, EvaluationSet, GateDiagnosticCode, MAX_DIAGNOSTICS, NumericPrediction,
    ScreeningPrediction,
};
use serde_json::Value;

const REVIEWED_FIXTURE: &str = include_str!("fixtures/evals/reviewed-small-v1.json");

fn fixture() -> EvaluationSet {
    EvaluationSet::from_json(REVIEWED_FIXTURE).expect("reviewed fixture should be valid")
}

fn rejection(
    result: Result<deepref_ai::EvaluationReport, EvaluationGateError>,
) -> Vec<GateDiagnosticCode> {
    match result {
        Err(EvaluationGateError::Rejected(error)) => {
            assert!(error.diagnostics().len() <= MAX_DIAGNOSTICS);
            assert!(
                error
                    .diagnostics()
                    .iter()
                    .all(|diagnostic| diagnostic.message.chars().count() <= 240)
            );
            error
                .diagnostics()
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect()
        }
        Err(EvaluationGateError::Evaluation(error)) => {
            panic!("mutation should be a gate rejection, not invalid input: {error:?}")
        }
        Ok(_) => panic!("mutation should fail the evaluation gate"),
    }
}

fn prediction_for<'a>(
    predictions: &'a mut [deepref_ai::ScreeningPredictionCase],
    case_id: &str,
) -> &'a mut deepref_ai::ScreeningPredictionCase {
    predictions
        .iter_mut()
        .find(|prediction| prediction.case_id.as_str() == case_id)
        .expect("fixture prediction case")
}

fn numeric_prediction_for<'a>(
    predictions: &'a mut [deepref_ai::NumericPredictionCase],
    case_id: &str,
) -> &'a mut deepref_ai::NumericPredictionCase {
    predictions
        .iter_mut()
        .find(|prediction| prediction.case_id.as_str() == case_id)
        .expect("fixture numeric prediction case")
}

fn citation_prediction_for<'a>(
    predictions: &'a mut [deepref_ai::CitationPredictionCase],
    case_id: &str,
) -> &'a mut deepref_ai::CitationPredictionCase {
    predictions
        .iter_mut()
        .find(|prediction| prediction.case_id.as_str() == case_id)
        .expect("fixture citation prediction case")
}

#[test]
fn reviewed_candidate_passes_and_metrics_use_conservative_screening_semantics() {
    let report = fixture()
        .evaluate_and_gate()
        .expect("current candidate should pass the checked-in gate");

    assert_eq!(report.candidate.screening.relevant, 3);
    assert_eq!(report.candidate.screening.irrelevant, 3);
    assert_eq!(report.candidate.screening.true_positives, 2);
    assert_eq!(report.candidate.screening.true_negatives, 1);
    assert_eq!(report.candidate.screening.false_positives, 1);
    assert_eq!(report.candidate.screening.false_negatives, 0);
    assert_eq!(report.candidate.screening.abstentions, 2);
    assert!((report.candidate.screening.sensitivity - (2.0 / 3.0)).abs() < f64::EPSILON);
    assert!((report.candidate.screening.specificity - (1.0 / 3.0)).abs() < f64::EPSILON);
    assert_eq!(report.candidate.screening.false_negative_rate, 0.0);
    assert!((report.candidate.screening.abstention_rate - (1.0 / 3.0)).abs() < f64::EPSILON);
    assert_eq!(report.candidate.screening.weighted_loss, 1.0);
    assert_eq!(report.baseline.screening.false_negatives, 0);
    assert_eq!(report.baseline.screening.weighted_loss, 1.0);
    assert_eq!(report.candidate.numeric.correct, 3);
    assert_eq!(report.candidate.citations.correct, 3);
    assert_eq!(report.baseline.numeric.correct, 1);
    assert_eq!(report.baseline.citations.correct, 2);
}

#[test]
fn false_exclusion_mutation_is_rejected_by_threshold_and_comparison() {
    let mut evaluation = fixture();
    prediction_for(
        &mut evaluation.candidate.screening_predictions,
        "screening-relevant-include-2",
    )
    .prediction = ScreeningPrediction::Exclude;

    let codes = rejection(evaluation.evaluate_and_gate());
    assert!(codes.contains(&GateDiagnosticCode::FalseNegativeRateAboveThreshold));
    assert!(codes.contains(&GateDiagnosticCode::FalseExclusionsRegressed));
}

#[test]
fn numeric_tolerance_failure_is_rejected_without_string_coercion() {
    let mut evaluation = fixture();
    numeric_prediction_for(
        &mut evaluation.candidate.numeric_predictions,
        "numeric-relative-tolerance-1",
    )
    .prediction = NumericPrediction::Value { value: 110.0 };

    let codes = rejection(evaluation.evaluate_and_gate());
    assert!(codes.contains(&GateDiagnosticCode::NumericAccuracyBelowThreshold));

    let mut json: Value = serde_json::from_str(REVIEWED_FIXTURE).expect("fixture JSON");
    json["candidate"]["numeric_predictions"][0]["prediction"]["value"] =
        Value::String("101.5".to_owned());
    let error = EvaluationSet::from_json(&json.to_string()).expect_err("strings are not numbers");
    assert!(matches!(error, deepref_ai::EvaluationError::Json { .. }));
}

#[test]
fn wrong_evidence_identity_or_hash_is_rejected_by_exact_citation_gate() {
    let mut evaluation = fixture();
    citation_prediction_for(
        &mut evaluation.candidate.citation_predictions,
        "citation-exact-single-1",
    )
    .citations = vec![deepref_ai::EvidenceIdentity {
        evidence_id: "fabricated-block".to_owned(),
        content_hash: "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_owned(),
    }];

    let codes = rejection(evaluation.evaluate_and_gate());
    assert!(codes.contains(&GateDiagnosticCode::CitationCorrectnessBelowThreshold));
}

#[test]
fn unnecessary_inclusion_regression_is_rejected_by_weighted_loss_comparison() {
    let mut evaluation = fixture();
    prediction_for(
        &mut evaluation.candidate.screening_predictions,
        "screening-irrelevant-exclude-1",
    )
    .prediction = ScreeningPrediction::Include;

    let codes = rejection(evaluation.evaluate_and_gate());
    assert!(codes.contains(&GateDiagnosticCode::SpecificityBelowThreshold));
    assert!(codes.contains(&GateDiagnosticCode::WeightedLossRegressed));
}

#[test]
fn schema_and_closed_predictions_reject_unknown_fixture_fields_with_bounded_error() {
    let mut json: Value = serde_json::from_str(REVIEWED_FIXTURE).expect("fixture JSON");
    json["candidate"]["unexpected"] = Value::Bool(true);
    let error = EvaluationSet::from_json(&json.to_string()).expect_err("unknown field");
    let message = error.to_string();
    assert!(message.chars().count() <= 300);

    let mut json: Value = serde_json::from_str(REVIEWED_FIXTURE).expect("fixture JSON");
    json["candidate"]["screening_predictions"][0]["prediction"] =
        Value::String("direct_state_command".to_owned());
    let error = EvaluationSet::from_json(&json.to_string()).expect_err("closed prediction");
    assert!(matches!(error, deepref_ai::EvaluationError::Json { .. }));
}
