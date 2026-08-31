use super::*;

pub(super) fn evaluate(
    evaluation: &EvaluationSet,
) -> Result<EvaluationReport, EvaluationGateError> {
    let report = evaluation.evaluate()?;
    let mut diagnostics = Vec::new();
    let candidate = &report.candidate;
    let thresholds = &evaluation.thresholds;

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
