//! Advisory domain and overall judgments derived from reviewer answers.
//!
//! RoB 2 version 2 follows the official algorithms in `rob2_official`. Version 1
//! keeps DeepRef's original rules, so assessments recorded against it still
//! validate. Either way the rules propose one judgment per domain, then an
//! overall judgment. Reviewers may override any suggestion, but must give a reason.

use serde::Serialize;
use serde_json::Value;

use super::{AppraisalDefinition, AppraisalDomain};

mod rob2_official;

pub const LOW_RISK: &str = "low_risk";
pub const SOME_CONCERNS: &str = "some_concerns";
pub const HIGH_RISK: &str = "high_risk";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DomainJudgmentSuggestion {
    pub domain_id: String,
    /// `None` when the answers for this domain are missing or not allowed.
    pub judgment: Option<String>,
    /// Question ids that set the domain judgment (empty for low risk).
    pub drivers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JudgmentSuggestion {
    pub domains: Vec<DomainJudgmentSuggestion>,
    /// `None` unless every domain has a suggestion.
    pub overall_judgment: Option<String>,
    /// Points the reviewer must check before accepting the overall judgment;
    /// empty when there is nothing to check.
    pub reviewer_notes: Vec<String>,
}

/// Suggests judgments for `definition` from `responses`.
///
/// Returns `None` when no rule set is registered for the definition id and
/// version, so generic definitions never receive a suggestion.
pub fn suggest_judgments(
    definition: &AppraisalDefinition,
    responses: &Value,
) -> Option<JudgmentSuggestion> {
    if definition.id.as_str() != RCT_ROB2_ID {
        return None;
    }
    match definition.version.get() {
        RCT_ROB2_VERSION => Some(rob2_official::suggest(definition, responses)),
        RCT_ROB2_V1_VERSION => Some(suggest_v1(definition, responses)),
        _ => None,
    }
}

/// DeepRef's original RoB 2 rules, used only by version 1 assessments.
fn suggest_v1(definition: &AppraisalDefinition, responses: &Value) -> JudgmentSuggestion {
    let answers = Answers(responses);
    let domains = definition
        .domains
        .iter()
        .map(|domain| rob2_domain_suggestion(domain, &answers))
        .collect::<Vec<_>>();
    let overall = domains
        .iter()
        .map(|domain| domain.judgment.as_deref().and_then(Risk::from_judgment))
        .collect::<Option<Vec<Risk>>>()
        .map(|risks| overall_risk(&risks).judgment().to_owned());
    JudgmentSuggestion {
        domains,
        overall_judgment: overall,
        reviewer_notes: Vec::new(),
    }
}

pub(super) const RCT_ROB2_ID: &str = "deepref-rct-rob2";
/// Current RoB 2 version: the official signalling questions and algorithms.
pub(super) const RCT_ROB2_VERSION: u32 = 2;
/// Earlier DeepRef wording and rules, kept for assessments already recorded.
const RCT_ROB2_V1_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Risk {
    Low,
    SomeConcerns,
    High,
}

impl Risk {
    const fn judgment(self) -> &'static str {
        match self {
            Self::Low => LOW_RISK,
            Self::SomeConcerns => SOME_CONCERNS,
            Self::High => HIGH_RISK,
        }
    }

    fn from_judgment(value: &str) -> Option<Self> {
        match value {
            LOW_RISK => Some(Self::Low),
            SOME_CONCERNS => Some(Self::SomeConcerns),
            HIGH_RISK => Some(Self::High),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Yes,
    ProbablyYes,
    ProbablyNo,
    No,
    NoInformation,
    NotApplicable,
}

impl Answer {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "yes" => Some(Self::Yes),
            "probably_yes" => Some(Self::ProbablyYes),
            "probably_no" => Some(Self::ProbablyNo),
            "no" => Some(Self::No),
            "no_information" => Some(Self::NoInformation),
            "not_applicable" => Some(Self::NotApplicable),
            _ => None,
        }
    }

    const fn is_favourable(self) -> bool {
        matches!(self, Self::Yes | Self::ProbablyYes)
    }
}

/// Reads answers by question id. Missing values, unknown values, and
/// "not applicable" on a question that does not allow it all count as absent.
struct Answers<'a>(&'a Value);

impl Answers<'_> {
    fn get(&self, question_id: &str, allows_not_applicable: bool) -> Option<Answer> {
        let answer = Answer::parse(self.0.get(question_id)?.as_str()?)?;
        (allows_not_applicable || answer != Answer::NotApplicable).then_some(answer)
    }
}

/// Risk for a question whose favourable answer is "yes".
/// A "no" on a critical question is high risk; elsewhere it is a concern.
fn risk_when_yes_is_favourable(answer: Answer, critical: bool) -> Risk {
    match answer {
        Answer::Yes | Answer::ProbablyYes | Answer::NotApplicable => Risk::Low,
        Answer::No if critical => Risk::High,
        Answer::No | Answer::ProbablyNo | Answer::NoInformation => Risk::SomeConcerns,
    }
}

/// Risk for a question where "yes" describes the concern.
/// A "yes" on a critical question is high risk; elsewhere it is a concern.
fn risk_when_yes_is_concern(answer: Answer, critical: bool) -> Risk {
    match answer {
        Answer::Yes if critical => Risk::High,
        Answer::Yes | Answer::ProbablyYes | Answer::NoInformation => Risk::SomeConcerns,
        Answer::No | Answer::ProbablyNo | Answer::NotApplicable => Risk::Low,
    }
}

/// Each finding is a risk level and the question that produced it.
type Findings = Vec<(Risk, &'static str)>;

struct DomainResult {
    risk: Risk,
    drivers: Vec<String>,
}

fn summarize(findings: &Findings) -> DomainResult {
    let risk = findings
        .iter()
        .map(|(risk, _)| *risk)
        .max()
        .unwrap_or(Risk::Low);
    let mut drivers: Vec<String> = Vec::new();
    if risk != Risk::Low {
        for (finding_risk, question_id) in findings {
            if *finding_risk == risk && !drivers.iter().any(|id| id == question_id) {
                drivers.push((*question_id).to_owned());
            }
        }
    }
    DomainResult { risk, drivers }
}

fn overall_risk(risks: &[Risk]) -> Risk {
    if risks.contains(&Risk::High) {
        Risk::High
    } else if risks.iter().all(|risk| *risk == Risk::Low) {
        Risk::Low
    } else {
        // Several "some concerns" stay at some concerns; reviewers may raise them.
        Risk::SomeConcerns
    }
}

fn rob2_domain_suggestion(
    domain: &AppraisalDomain,
    answers: &Answers<'_>,
) -> DomainJudgmentSuggestion {
    let findings = match domain.id.as_str() {
        "randomization_process" => randomization_findings(answers),
        "deviations_from_intervention" => deviations_findings(answers),
        "missing_outcome_data" => missing_data_findings(answers),
        "outcome_measurement" => measurement_findings(answers),
        "reported_result" => reported_result_findings(answers),
        _ => None,
    };
    let result = findings.as_ref().map(summarize);
    DomainJudgmentSuggestion {
        domain_id: domain.id.clone(),
        judgment: result
            .as_ref()
            .map(|result| result.risk.judgment().to_owned()),
        drivers: result.map(|result| result.drivers).unwrap_or_default(),
    }
}

/// Allocation: a predictable sequence is high risk; so is an imbalance at
/// baseline unless allocation was concealed, when it is only a concern.
fn randomization_findings(answers: &Answers<'_>) -> Option<Findings> {
    let sequence = answers.get("sequence_unpredictable", false)?;
    let concealed = answers.get("allocation_concealed", false)?;
    let baseline = answers.get("baseline_comparable", false)?;
    let baseline_risk = match risk_when_yes_is_favourable(baseline, true) {
        Risk::High if concealed.is_favourable() => Risk::SomeConcerns,
        risk => risk,
    };
    Some(vec![
        (
            risk_when_yes_is_favourable(sequence, true),
            "sequence_unpredictable",
        ),
        (
            risk_when_yes_is_favourable(concealed, false),
            "allocation_concealed",
        ),
        (baseline_risk, "baseline_comparable"),
    ])
}

/// Deviations: unbalanced or influential departures are high risk unless the
/// analysis still keeps the randomized groups intact.
fn deviations_findings(answers: &Answers<'_>) -> Option<Findings> {
    let blinded = answers.get("participants_and_staff_blinded", false)?;
    let deviations = answers.get("deviations_unbalanced", false)?;
    let analysis = answers.get("analysis_by_assigned_arm", false)?;
    let adherence = answers.get("adherence_analysis_appropriate", true)?;
    let mut findings = vec![
        (
            risk_when_yes_is_favourable(blinded, false),
            "participants_and_staff_blinded",
        ),
        (
            risk_when_yes_is_concern(deviations, false),
            "deviations_unbalanced",
        ),
        (
            risk_when_yes_is_favourable(analysis, true),
            "analysis_by_assigned_arm",
        ),
        (
            risk_when_yes_is_favourable(adherence, false),
            "adherence_analysis_appropriate",
        ),
    ];
    if deviations == Answer::Yes && !analysis.is_favourable() {
        findings.push((Risk::High, "deviations_unbalanced"));
        findings.push((Risk::High, "analysis_by_assigned_arm"));
    }
    Some(findings)
}

/// Missing data: data that are missing and plausibly linked to the outcome
/// are high risk unless a justified method was used.
fn missing_data_findings(answers: &Answers<'_>) -> Option<Findings> {
    let complete = answers.get("outcome_data_nearly_complete", false)?;
    let related = answers.get("missingness_related_to_outcome", false)?;
    let method = answers.get("missing_data_method", true)?;
    let method_justified = matches!(
        method,
        Answer::Yes | Answer::ProbablyYes | Answer::NotApplicable
    );
    let related_risk = if related == Answer::Yes && !method_justified {
        Risk::High
    } else {
        risk_when_yes_is_concern(related, false)
    };
    Some(vec![
        (
            risk_when_yes_is_favourable(complete, false),
            "outcome_data_nearly_complete",
        ),
        (related_risk, "missingness_related_to_outcome"),
        (
            risk_when_yes_is_favourable(method, false),
            "missing_data_method",
        ),
    ])
}

/// Measurement: unequal measurement is high risk; so is an unblinded
/// assessor for a subjective outcome (not applicable marks an objective one).
fn measurement_findings(answers: &Answers<'_>) -> Option<Findings> {
    let equivalent = answers.get("measurement_equivalent", false)?;
    let unaware = answers.get("assessors_unaware", true)?;
    Some(vec![
        (
            risk_when_yes_is_favourable(equivalent, true),
            "measurement_equivalent",
        ),
        (
            risk_when_yes_is_favourable(unaware, true),
            "assessors_unaware",
        ),
    ])
}

/// Reported result: a reported result chosen for significance is high risk;
/// several options with no prior plan is high risk too.
fn reported_result_findings(answers: &Answers<'_>) -> Option<Findings> {
    let planned = answers.get("analysis_planned_in_advance", false)?;
    let alternatives = answers.get("alternatives_available", false)?;
    let selected = answers.get("selected_by_significance", false)?;
    let mut findings = vec![
        (
            risk_when_yes_is_favourable(planned, false),
            "analysis_planned_in_advance",
        ),
        (
            risk_when_yes_is_concern(alternatives, false),
            "alternatives_available",
        ),
        (
            risk_when_yes_is_concern(selected, true),
            "selected_by_significance",
        ),
    ];
    if alternatives == Answer::Yes && !planned.is_favourable() {
        findings.push((Risk::High, "alternatives_available"));
        findings.push((Risk::High, "analysis_planned_in_advance"));
    }
    Some(findings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appraisal::get_appraisal_definition;
    use serde_json::json;

    const FAVOURABLE_RESPONSES: [(&str, &str); 14] = [
        ("sequence_unpredictable", "yes"),
        ("allocation_concealed", "yes"),
        ("baseline_comparable", "yes"),
        ("participants_and_staff_blinded", "yes"),
        ("deviations_unbalanced", "no"),
        ("analysis_by_assigned_arm", "yes"),
        ("adherence_analysis_appropriate", "not_applicable"),
        ("outcome_data_nearly_complete", "yes"),
        ("missingness_related_to_outcome", "no"),
        ("missing_data_method", "not_applicable"),
        ("measurement_equivalent", "yes"),
        ("assessors_unaware", "not_applicable"),
        ("analysis_planned_in_advance", "yes"),
        ("alternatives_available", "no"),
    ];

    fn responses(overrides: &[(&str, &str)]) -> Value {
        let mut map = serde_json::Map::new();
        for (question, value) in FAVOURABLE_RESPONSES
            .iter()
            .chain(&[("selected_by_significance", "no")])
        {
            map.insert((*question).to_owned(), json!(value));
        }
        for (question, value) in overrides {
            map.insert((*question).to_owned(), json!(value));
        }
        Value::Object(map)
    }

    fn suggest(overrides: &[(&str, &str)]) -> JudgmentSuggestion {
        let definition = get_appraisal_definition(RCT_ROB2_ID, RCT_ROB2_V1_VERSION).unwrap();
        suggest_judgments(&definition, &responses(overrides)).expect("rob2 has rules")
    }

    fn domain<'a>(suggestion: &'a JudgmentSuggestion, id: &str) -> &'a DomainJudgmentSuggestion {
        suggestion
            .domains
            .iter()
            .find(|domain| domain.domain_id == id)
            .expect("domain present")
    }

    #[test]
    fn favourable_answers_give_low_risk_everywhere() {
        let suggestion = suggest(&[]);
        assert!(
            suggestion
                .domains
                .iter()
                .all(|domain| domain.judgment.as_deref() == Some(LOW_RISK)
                    && domain.drivers.is_empty())
        );
        assert_eq!(suggestion.overall_judgment.as_deref(), Some(LOW_RISK));
    }

    #[test]
    fn a_high_risk_domain_makes_the_overall_judgment_high() {
        let suggestion = suggest(&[("sequence_unpredictable", "no")]);
        let randomization = domain(&suggestion, "randomization_process");
        assert_eq!(randomization.judgment.as_deref(), Some(HIGH_RISK));
        assert_eq!(randomization.drivers, vec!["sequence_unpredictable"]);
        assert_eq!(suggestion.overall_judgment.as_deref(), Some(HIGH_RISK));
    }

    #[test]
    fn a_single_concern_is_some_concerns_overall_without_raising_to_high() {
        let suggestion = suggest(&[("participants_and_staff_blinded", "no")]);
        let deviations = domain(&suggestion, "deviations_from_intervention");
        assert_eq!(deviations.judgment.as_deref(), Some(SOME_CONCERNS));
        assert_eq!(deviations.drivers, vec!["participants_and_staff_blinded"]);
        assert_eq!(suggestion.overall_judgment.as_deref(), Some(SOME_CONCERNS));
    }

    #[test]
    fn several_concerns_in_different_domains_stay_some_concerns() {
        let suggestion = suggest(&[
            ("allocation_concealed", "probably_no"),
            ("outcome_data_nearly_complete", "no_information"),
            ("selected_by_significance", "no_information"),
        ]);
        let judgments = suggestion
            .domains
            .iter()
            .map(|domain| domain.judgment.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(
            judgments,
            vec![
                Some(SOME_CONCERNS),
                Some(LOW_RISK),
                Some(SOME_CONCERNS),
                Some(LOW_RISK),
                Some(SOME_CONCERNS),
            ]
        );
        assert_eq!(suggestion.overall_judgment.as_deref(), Some(SOME_CONCERNS));
    }

    #[test]
    fn baseline_imbalance_is_high_only_without_concealment() {
        let unconcealed = suggest(&[
            ("baseline_comparable", "no"),
            ("allocation_concealed", "no"),
        ]);
        assert_eq!(
            domain(&unconcealed, "randomization_process")
                .judgment
                .as_deref(),
            Some(HIGH_RISK)
        );
        let concealed = suggest(&[("baseline_comparable", "no")]);
        assert_eq!(
            domain(&concealed, "randomization_process")
                .judgment
                .as_deref(),
            Some(SOME_CONCERNS)
        );
    }

    #[test]
    fn unbalanced_deviations_are_high_when_the_analysis_does_not_keep_arms() {
        let high = suggest(&[
            ("deviations_unbalanced", "yes"),
            ("analysis_by_assigned_arm", "probably_no"),
        ]);
        let deviations = domain(&high, "deviations_from_intervention");
        assert_eq!(deviations.judgment.as_deref(), Some(HIGH_RISK));
        assert_eq!(
            deviations.drivers,
            vec!["deviations_unbalanced", "analysis_by_assigned_arm"]
        );

        let tolerated = suggest(&[("deviations_unbalanced", "yes")]);
        assert_eq!(
            domain(&tolerated, "deviations_from_intervention")
                .judgment
                .as_deref(),
            Some(SOME_CONCERNS)
        );
    }

    #[test]
    fn missing_data_linked_to_the_outcome_is_high_without_a_method() {
        let high = suggest(&[
            ("missingness_related_to_outcome", "yes"),
            ("missing_data_method", "no"),
        ]);
        assert_eq!(
            domain(&high, "missing_outcome_data").judgment.as_deref(),
            Some(HIGH_RISK)
        );
        let justified = suggest(&[
            ("missingness_related_to_outcome", "yes"),
            ("missing_data_method", "probably_yes"),
        ]);
        assert_eq!(
            domain(&justified, "missing_outcome_data")
                .judgment
                .as_deref(),
            Some(SOME_CONCERNS)
        );
    }

    #[test]
    fn unblinded_assessors_are_high_only_when_the_outcome_is_subjective() {
        let subjective = suggest(&[("assessors_unaware", "no")]);
        assert_eq!(
            domain(&subjective, "outcome_measurement")
                .judgment
                .as_deref(),
            Some(HIGH_RISK)
        );
        let objective = suggest(&[("assessors_unaware", "not_applicable")]);
        assert_eq!(
            domain(&objective, "outcome_measurement")
                .judgment
                .as_deref(),
            Some(LOW_RISK)
        );
    }

    #[test]
    fn outcome_chosen_for_significance_is_high_risk() {
        let suggestion = suggest(&[("selected_by_significance", "yes")]);
        let reported = domain(&suggestion, "reported_result");
        assert_eq!(reported.judgment.as_deref(), Some(HIGH_RISK));
        assert_eq!(reported.drivers, vec!["selected_by_significance"]);
        assert_eq!(suggestion.overall_judgment.as_deref(), Some(HIGH_RISK));
    }

    #[test]
    fn several_options_without_a_prior_plan_is_high_risk() {
        let planned = suggest(&[
            ("alternatives_available", "yes"),
            ("analysis_planned_in_advance", "no_information"),
        ]);
        assert_eq!(
            domain(&planned, "reported_result").judgment.as_deref(),
            Some(HIGH_RISK)
        );
        let with_plan = suggest(&[
            ("alternatives_available", "yes"),
            ("analysis_planned_in_advance", "yes"),
        ]);
        assert_eq!(
            domain(&with_plan, "reported_result").judgment.as_deref(),
            Some(SOME_CONCERNS)
        );
    }

    #[test]
    fn missing_or_misplaced_answers_leave_the_domain_and_overall_unsuggested() {
        let definition = get_appraisal_definition(RCT_ROB2_ID, RCT_ROB2_V1_VERSION).unwrap();
        let mut partial = responses(&[]);
        partial
            .as_object_mut()
            .unwrap()
            .remove("baseline_comparable");
        let suggestion = suggest_judgments(&definition, &partial).unwrap();
        assert_eq!(domain(&suggestion, "randomization_process").judgment, None);
        assert_eq!(suggestion.overall_judgment, None);
        assert_eq!(
            domain(&suggestion, "reported_result").judgment.as_deref(),
            Some(LOW_RISK)
        );

        let misplaced = suggest(&[("analysis_by_assigned_arm", "not_applicable")]);
        assert_eq!(
            domain(&misplaced, "deviations_from_intervention").judgment,
            None
        );
        assert_eq!(misplaced.overall_judgment, None);
    }

    #[test]
    fn generic_definitions_get_no_suggestion() {
        let definition = get_appraisal_definition("deepref-rct-generic", 1).unwrap();
        assert_eq!(
            suggest_judgments(&definition, &json!({"allocation_description": "yes"})),
            None
        );
    }

    #[test]
    fn every_rule_targets_shipped_domains_questions_and_judgment_values() {
        let definition = get_appraisal_definition(RCT_ROB2_ID, RCT_ROB2_V1_VERSION).unwrap();
        let suggestion = suggest_judgments(&definition, &responses(&[])).unwrap();
        assert_eq!(suggestion.domains.len(), definition.domains.len());
        let judgment_values = definition
            .overall_judgment
            .options
            .iter()
            .map(|option| option.value.as_str())
            .collect::<Vec<_>>();
        for domain in &definition.domains {
            assert!(
                domain
                    .judgment
                    .options
                    .iter()
                    .any(|option| option.value == LOW_RISK)
                    && domain
                        .judgment
                        .options
                        .iter()
                        .any(|option| option.value == HIGH_RISK)
                    && domain
                        .judgment
                        .options
                        .iter()
                        .any(|option| option.value == SOME_CONCERNS),
                "domain {} must offer all three judgments",
                domain.id
            );
            assert!(
                suggestion
                    .domains
                    .iter()
                    .any(|result| result.domain_id == domain.id && result.judgment.is_some()),
                "domain {} must have a rule",
                domain.id
            );
        }
        for value in [LOW_RISK, SOME_CONCERNS, HIGH_RISK] {
            assert!(judgment_values.contains(&value));
        }
        for (question_id, value) in FAVOURABLE_RESPONSES {
            assert!(
                definition
                    .domains
                    .iter()
                    .flat_map(|domain| domain.questions.iter())
                    .any(|question| question.id == question_id),
                "rule question {question_id} must exist in the definition ({value})"
            );
        }
    }
}
