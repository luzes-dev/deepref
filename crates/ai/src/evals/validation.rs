use std::collections::BTreeSet;

use crate::is_sha256;

use super::*;

pub(super) fn validate(evaluation: &EvaluationSet) -> Result<(), EvaluationValidationError> {
    let mut diagnostics = Vec::new();
    if evaluation.schema != EVALUATION_SCHEMA {
        add_validation(
            &mut diagnostics,
            format!("schema must be {EVALUATION_SCHEMA}"),
        );
    }
    if evaluation.version != EVALUATION_VERSION {
        add_validation(
            &mut diagnostics,
            format!("version must be {EVALUATION_VERSION}"),
        );
    }
    validate_id(&mut diagnostics, "gold_set_id", &evaluation.gold_set_id);
    validate_text(
        &mut diagnostics,
        "reviewer.reviewer_id",
        &evaluation.reviewer.reviewer_id,
        MAX_TEXT_LENGTH,
    );
    validate_text(
        &mut diagnostics,
        "reviewer.review_notes",
        &evaluation.reviewer.review_notes,
        4_000,
    );
    validate_thresholds(&mut diagnostics, &evaluation.thresholds);

    if evaluation.screening_cases.is_empty() {
        add_validation(
            &mut diagnostics,
            "screening_cases must not be empty".to_owned(),
        );
    }
    if evaluation.numeric_extraction_cases.is_empty() {
        add_validation(
            &mut diagnostics,
            "numeric_extraction_cases must not be empty".to_owned(),
        );
    }
    if evaluation.citation_cases.is_empty() {
        add_validation(
            &mut diagnostics,
            "citation_cases must not be empty".to_owned(),
        );
    }
    if evaluation.screening_cases.len() > MAX_CASES
        || evaluation.numeric_extraction_cases.len() > MAX_CASES
        || evaluation.citation_cases.len() > MAX_CASES
    {
        add_validation(
            &mut diagnostics,
            "evaluation contains too many cases".to_owned(),
        );
    }

    let mut all_case_ids = BTreeSet::new();
    for case in &evaluation.screening_cases {
        validate_case_id(&mut diagnostics, &mut all_case_ids, "screening", &case.id);
    }
    for case in &evaluation.numeric_extraction_cases {
        validate_case_id(&mut diagnostics, &mut all_case_ids, "numeric", &case.id);
        validate_numeric_gold(&mut diagnostics, case);
    }
    for case in &evaluation.citation_cases {
        validate_case_id(&mut diagnostics, &mut all_case_ids, "citation", &case.id);
        validate_evidence_list(
            &mut diagnostics,
            "citation gold evidence",
            &case.expected_evidence,
            true,
        );
    }

    let screening_ids = evaluation
        .screening_cases
        .iter()
        .map(|case| case.id.clone())
        .collect::<BTreeSet<_>>();
    let numeric_ids = evaluation
        .numeric_extraction_cases
        .iter()
        .map(|case| case.id.clone())
        .collect::<BTreeSet<_>>();
    let citation_ids = evaluation
        .citation_cases
        .iter()
        .map(|case| case.id.clone())
        .collect::<BTreeSet<_>>();
    validate_model(
        &mut diagnostics,
        "baseline",
        &evaluation.baseline,
        &screening_ids,
        &numeric_ids,
        &citation_ids,
    );
    validate_model(
        &mut diagnostics,
        "candidate",
        &evaluation.candidate,
        &screening_ids,
        &numeric_ids,
        &citation_ids,
    );

    if !evaluation
        .screening_cases
        .iter()
        .any(|case| matches!(case.truth, ScreeningTruth::Relevant))
    {
        add_validation(
            &mut diagnostics,
            "screening gold set needs a relevant case".to_owned(),
        );
    }
    if !evaluation
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

fn add_validation(diagnostics: &mut Vec<String>, message: String) {
    if diagnostics.len() < MAX_DIAGNOSTICS {
        diagnostics.push(bounded_text(message, MAX_DIAGNOSTIC_LENGTH));
    }
}

fn bounded_text(value: String, max_length: usize) -> String {
    value.chars().take(max_length).collect()
}
