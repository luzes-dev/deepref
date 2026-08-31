use std::collections::BTreeSet;

use super::*;

pub(super) fn evaluate(evaluation: &EvaluationSet) -> Result<EvaluationReport, EvaluationError> {
    evaluation.validate()?;
    Ok(EvaluationReport {
        baseline: evaluate_model(
            &evaluation.baseline,
            &evaluation.screening_cases,
            &evaluation.numeric_extraction_cases,
            &evaluation.citation_cases,
            &evaluation.thresholds,
        )?,
        candidate: evaluate_model(
            &evaluation.candidate,
            &evaluation.screening_cases,
            &evaluation.numeric_extraction_cases,
            &evaluation.citation_cases,
            &evaluation.thresholds,
        )?,
    })
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

fn bounded_text(value: String, max_length: usize) -> String {
    value.chars().take(max_length).collect()
}
