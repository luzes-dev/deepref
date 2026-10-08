//! Suggested judgments for the official RoB 2 tool, parallel-group randomized
//! trials, assignment-to-intervention variant (Sterne JAC, et al. BMJ 2019;366:l4898;
//! RoB 2 guidance, 22 August 2019).
//!
//! Each domain follows its decision tree (guidance Figures 1, 2, 4, 5 and 7, mapped
//! in Tables 4, 6, 10, 12 and 14). A domain stops at the first answer that settles
//! its judgment. Its drivers are the consulted questions whose answers lean towards
//! concern, so a low-risk judgment has none. Overall judgment follows Table 1.

use serde_json::Value;

use super::{Answer, Answers, DomainJudgmentSuggestion, JudgmentSuggestion, Risk, overall_risk};
use crate::appraisal::AppraisalDefinition;

const RANDOMIZATION_PROCESS: &str = "randomization_process";
const DEVIATIONS_FROM_INTERVENTION: &str = "deviations_from_intervention";
const MISSING_OUTCOME_DATA: &str = "missing_outcome_data";
const OUTCOME_MEASUREMENT: &str = "outcome_measurement";
const REPORTED_RESULT: &str = "reported_result";

const SEQUENCE_RANDOM: &str = "sequence_random";
const ALLOCATION_CONCEALED: &str = "allocation_concealed";
const BASELINE_IMBALANCE: &str = "baseline_imbalance";
const PARTICIPANTS_AWARE: &str = "participants_aware";
const PERSONNEL_AWARE: &str = "personnel_aware";
const TRIAL_CONTEXT_DEVIATIONS: &str = "trial_context_deviations";
const DEVIATIONS_AFFECTED_OUTCOME: &str = "deviations_affected_outcome";
const DEVIATIONS_BALANCED: &str = "deviations_balanced";
const ANALYSIS_APPROPRIATE: &str = "analysis_appropriate";
const SWITCHING_SUBSTANTIAL_IMPACT: &str = "switching_substantial_impact";
const OUTCOME_DATA_COMPLETE: &str = "outcome_data_complete";
const MISSING_DATA_NOT_BIASED: &str = "missing_data_not_biased";
const MISSINGNESS_COULD_DEPEND: &str = "missingness_could_depend";
const MISSINGNESS_LIKELY_DEPENDS: &str = "missingness_likely_depends";
const MEASUREMENT_INAPPROPRIATE: &str = "measurement_inappropriate";
const MEASUREMENT_DIFFERED: &str = "measurement_differed";
const ASSESSORS_AWARE: &str = "assessors_aware";
const ASSESSMENT_COULD_BE_INFLUENCED: &str = "assessment_could_be_influenced";
const ASSESSMENT_LIKELY_INFLUENCED: &str = "assessment_likely_influenced";
const ANALYSIS_PRESPECIFIED: &str = "analysis_prespecified";
const OUTCOME_MEASUREMENT_SELECTED: &str = "outcome_measurement_selected";
const ANALYSIS_SELECTED: &str = "analysis_selected";

/// A consulted question and whether its answer leans towards concern.
type Step = (&'static str, bool);

#[derive(Debug, Clone, PartialEq, Eq)]
struct Outcome {
    risk: Risk,
    drivers: Vec<&'static str>,
}

impl Outcome {
    fn low() -> Self {
        Self {
            risk: Risk::Low,
            drivers: Vec::new(),
        }
    }

    /// Low risk carries no drivers; a concern carries the steps that leaned that way.
    fn judged(risk: Risk, steps: &[Step]) -> Self {
        if risk == Risk::Low {
            return Self::low();
        }
        Self {
            risk,
            drivers: steps
                .iter()
                .filter(|(_, concern)| *concern)
                .map(|(id, _)| *id)
                .collect(),
        }
    }
}

const fn is_yes(answer: Answer) -> bool {
    matches!(answer, Answer::Yes | Answer::ProbablyYes)
}

const fn is_no(answer: Answer) -> bool {
    matches!(answer, Answer::No | Answer::ProbablyNo)
}

/// Suggests a judgment for every domain of `definition` and the overall judgment.
pub(super) fn suggest(definition: &AppraisalDefinition, responses: &Value) -> JudgmentSuggestion {
    let answers = Answers(responses);
    let outcomes = definition
        .domains
        .iter()
        .map(|domain| (domain, domain_outcome(&domain.id, &answers)))
        .collect::<Vec<_>>();
    let risks = outcomes
        .iter()
        .map(|(_, outcome)| outcome.as_ref().map(|outcome| outcome.risk))
        .collect::<Option<Vec<Risk>>>();
    let overall = risks.as_deref().map(overall_risk);
    let reviewer_notes = match overall {
        Some(Risk::SomeConcerns) => some_concerns_note(&outcomes),
        _ => Vec::new(),
    };
    JudgmentSuggestion {
        domains: outcomes
            .into_iter()
            .map(|(domain, outcome)| DomainJudgmentSuggestion {
                domain_id: domain.id.clone(),
                judgment: outcome
                    .as_ref()
                    .map(|outcome| outcome.risk.judgment().to_owned()),
                drivers: outcome
                    .map(|outcome| {
                        outcome
                            .drivers
                            .into_iter()
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            })
            .collect(),
        overall_judgment: overall.map(|risk| risk.judgment().to_owned()),
        reviewer_notes,
    }
}

fn domain_outcome(domain_id: &str, answers: &Answers<'_>) -> Option<Outcome> {
    match domain_id {
        RANDOMIZATION_PROCESS => randomization(answers),
        DEVIATIONS_FROM_INTERVENTION => deviations_assignment(answers),
        MISSING_OUTCOME_DATA => missing_outcome_data(answers),
        OUTCOME_MEASUREMENT => outcome_measurement(answers),
        REPORTED_RESULT => reported_result(answers),
        _ => None,
    }
}

/// Domain 1 (guidance Figure 1, Table 4). Concealment decides first; a
/// sequence that is not random is a concern; an unknown concealment depends on the baseline.
fn randomization(answers: &Answers<'_>) -> Option<Outcome> {
    let sequence = answers.get(SEQUENCE_RANDOM, false)?;
    let concealed = answers.get(ALLOCATION_CONCEALED, false)?;
    let imbalance = answers.get(BASELINE_IMBALANCE, false)?;
    if is_no(concealed) {
        return Some(Outcome::judged(Risk::High, &[(ALLOCATION_CONCEALED, true)]));
    }
    if is_yes(concealed) {
        if is_no(sequence) {
            return Some(Outcome::judged(
                Risk::SomeConcerns,
                &[(ALLOCATION_CONCEALED, false), (SEQUENCE_RANDOM, true)],
            ));
        }
        let risk = if is_yes(imbalance) {
            Risk::SomeConcerns
        } else {
            Risk::Low
        };
        return Some(Outcome::judged(
            risk,
            &[
                (ALLOCATION_CONCEALED, false),
                (SEQUENCE_RANDOM, false),
                (BASELINE_IMBALANCE, is_yes(imbalance)),
            ],
        ));
    }
    // Concealment is not reported: a baseline problem makes it high risk.
    let risk = if is_yes(imbalance) {
        Risk::High
    } else {
        Risk::SomeConcerns
    };
    Some(Outcome::judged(
        risk,
        &[
            (ALLOCATION_CONCEALED, true),
            (BASELINE_IMBALANCE, is_yes(imbalance)),
        ],
    ))
}

/// Domain 2, effect of assignment (guidance Tables 5 and 6, Figure 2): Part 1
/// covers awareness and deviations (2.1-2.5), Part 2 the analysis (2.6-2.7).
fn deviations_assignment(answers: &Answers<'_>) -> Option<Outcome> {
    let part_one = assignment_part_one(answers)?;
    let part_two = assignment_part_two(answers)?;
    Some(combine(&[part_one, part_two]))
}

fn assignment_part_one(answers: &Answers<'_>) -> Option<Outcome> {
    let participants = answers.get(PARTICIPANTS_AWARE, false)?;
    let personnel = answers.get(PERSONNEL_AWARE, false)?;
    if is_no(participants) && is_no(personnel) {
        return Some(Outcome::low());
    }
    // 2.3 is asked because 2.1 or 2.2 is yes, probably yes or no information.
    let context = answers.get(TRIAL_CONTEXT_DEVIATIONS, false)?;
    let mut steps = vec![
        (PARTICIPANTS_AWARE, !is_no(participants)),
        (PERSONNEL_AWARE, !is_no(personnel)),
        (TRIAL_CONTEXT_DEVIATIONS, !is_no(context)),
    ];
    if is_no(context) {
        return Some(Outcome::judged(Risk::Low, &steps));
    }
    if matches!(context, Answer::NoInformation) {
        return Some(Outcome::judged(Risk::SomeConcerns, &steps));
    }
    // 2.3 is yes or probably yes: 2.4 says whether the deviations could affect the outcome.
    let affected = answers.get(DEVIATIONS_AFFECTED_OUTCOME, false)?;
    steps.push((DEVIATIONS_AFFECTED_OUTCOME, !is_no(affected)));
    if is_no(affected) {
        return Some(Outcome::judged(Risk::SomeConcerns, &steps));
    }
    // 2.4 is yes, probably yes or no information: unbalanced deviations are high risk.
    let balanced = answers.get(DEVIATIONS_BALANCED, false)?;
    steps.push((DEVIATIONS_BALANCED, !is_yes(balanced)));
    let risk = if is_yes(balanced) {
        Risk::SomeConcerns
    } else {
        Risk::High
    };
    Some(Outcome::judged(risk, &steps))
}

fn assignment_part_two(answers: &Answers<'_>) -> Option<Outcome> {
    let analysis = answers.get(ANALYSIS_APPROPRIATE, false)?;
    if is_yes(analysis) {
        return Some(Outcome::low());
    }
    // 2.6 is no, probably no or no information: 2.7 decides between some concerns and high risk.
    let impact = answers.get(SWITCHING_SUBSTANTIAL_IMPACT, false)?;
    let steps = [
        (ANALYSIS_APPROPRIATE, true),
        (SWITCHING_SUBSTANTIAL_IMPACT, !is_no(impact)),
    ];
    let risk = if is_no(impact) {
        Risk::SomeConcerns
    } else {
        Risk::High
    };
    Some(Outcome::judged(risk, &steps))
}

/// Domain 3 (guidance Table 10, Figure 4). The answer to 3.2 has no
/// "no information" option, so it is treated as unanswered.
fn missing_outcome_data(answers: &Answers<'_>) -> Option<Outcome> {
    let complete = answers.get(OUTCOME_DATA_COMPLETE, false)?;
    if is_yes(complete) {
        return Some(Outcome::low());
    }
    let not_biased = answers.get(MISSING_DATA_NOT_BIASED, false)?;
    if matches!(not_biased, Answer::NoInformation) {
        return None;
    }
    if is_yes(not_biased) {
        return Some(Outcome::low());
    }
    let could_depend = answers.get(MISSINGNESS_COULD_DEPEND, false)?;
    let mut steps = vec![
        (OUTCOME_DATA_COMPLETE, true),
        (MISSING_DATA_NOT_BIASED, true),
        (MISSINGNESS_COULD_DEPEND, !is_no(could_depend)),
    ];
    if is_no(could_depend) {
        return Some(Outcome::judged(Risk::Low, &steps));
    }
    let likely = answers.get(MISSINGNESS_LIKELY_DEPENDS, false)?;
    steps.push((MISSINGNESS_LIKELY_DEPENDS, !is_no(likely)));
    let risk = if is_no(likely) {
        Risk::SomeConcerns
    } else {
        Risk::High
    };
    Some(Outcome::judged(risk, &steps))
}

/// Domain 4 (guidance Table 12, Figure 5). An unknown difference between groups
/// is a concern, and an inappropriate or differing measurement is high risk.
fn outcome_measurement(answers: &Answers<'_>) -> Option<Outcome> {
    let inappropriate = answers.get(MEASUREMENT_INAPPROPRIATE, false)?;
    let differed = answers.get(MEASUREMENT_DIFFERED, false)?;
    if is_yes(inappropriate) || is_yes(differed) {
        return Some(Outcome::judged(
            Risk::High,
            &[
                (MEASUREMENT_INAPPROPRIATE, is_yes(inappropriate)),
                (MEASUREMENT_DIFFERED, is_yes(differed)),
            ],
        ));
    }
    let differs_unknown = !is_no(differed);
    let mut steps = vec![
        (MEASUREMENT_INAPPROPRIATE, false),
        (MEASUREMENT_DIFFERED, differs_unknown),
    ];
    let aware = answers.get(ASSESSORS_AWARE, false)?;
    steps.push((ASSESSORS_AWARE, !is_no(aware)));
    if is_no(aware) {
        let risk = if differs_unknown {
            Risk::SomeConcerns
        } else {
            Risk::Low
        };
        return Some(Outcome::judged(risk, &steps));
    }
    let could_influence = answers.get(ASSESSMENT_COULD_BE_INFLUENCED, false)?;
    steps.push((ASSESSMENT_COULD_BE_INFLUENCED, !is_no(could_influence)));
    if is_no(could_influence) {
        let risk = if differs_unknown {
            Risk::SomeConcerns
        } else {
            Risk::Low
        };
        return Some(Outcome::judged(risk, &steps));
    }
    let likely = answers.get(ASSESSMENT_LIKELY_INFLUENCED, false)?;
    steps.push((ASSESSMENT_LIKELY_INFLUENCED, !is_no(likely)));
    let risk = if is_no(likely) {
        Risk::SomeConcerns
    } else {
        Risk::High
    };
    Some(Outcome::judged(risk, &steps))
}

/// Domain 5 (guidance Table 14, Figure 7). Selection on the results is high
/// risk; a missing plan or missing information is a concern.
fn reported_result(answers: &Answers<'_>) -> Option<Outcome> {
    let prespecified = answers.get(ANALYSIS_PRESPECIFIED, false)?;
    let outcome_selected = answers.get(OUTCOME_MEASUREMENT_SELECTED, false)?;
    let analysis_selected = answers.get(ANALYSIS_SELECTED, false)?;
    if is_yes(outcome_selected) || is_yes(analysis_selected) {
        return Some(Outcome::judged(
            Risk::High,
            &[
                (OUTCOME_MEASUREMENT_SELECTED, is_yes(outcome_selected)),
                (ANALYSIS_SELECTED, is_yes(analysis_selected)),
            ],
        ));
    }
    let steps = [
        (ANALYSIS_PRESPECIFIED, !is_yes(prespecified)),
        (OUTCOME_MEASUREMENT_SELECTED, !is_no(outcome_selected)),
        (ANALYSIS_SELECTED, !is_no(analysis_selected)),
    ];
    let low = is_yes(prespecified) && is_no(outcome_selected) && is_no(analysis_selected);
    let risk = if low { Risk::Low } else { Risk::SomeConcerns };
    Some(Outcome::judged(risk, &steps))
}

/// Combines the parts of a domain (Table 6): high if any part is high, some
/// concerns if any part is, otherwise low. Drivers come from the parts at that level.
fn combine(parts: &[Outcome]) -> Outcome {
    let risk = parts
        .iter()
        .map(|part| part.risk)
        .max()
        .unwrap_or(Risk::Low);
    let drivers = parts
        .iter()
        .filter(|part| part.risk == risk)
        .flat_map(|part| part.drivers.iter().copied())
        .collect::<Vec<_>>();
    if risk == Risk::Low {
        Outcome::low()
    } else {
        Outcome { risk, drivers }
    }
}

/// Table 1 leaves overall judgment to the reviewer when several domains have some
/// concerns. The suggestion stays at some concerns, with a note to check it.
fn some_concerns_note(
    outcomes: &[(&crate::appraisal::AppraisalDomain, Option<Outcome>)],
) -> Vec<String> {
    let concern_labels = outcomes
        .iter()
        .filter(|(_, outcome)| {
            outcome
                .as_ref()
                .is_some_and(|outcome| outcome.risk == Risk::SomeConcerns)
        })
        .map(|(domain, _)| domain.label.as_str())
        .collect::<Vec<_>>();
    if concern_labels.len() < 2 {
        return Vec::new();
    }
    vec![format!(
        "Some concerns in {} domains ({}). Table 1 of the RoB 2 guidance allows an overall high \
         risk of bias when such concerns together substantially lower confidence in the result. \
         The suggestion stays at Some concerns; the reviewer decides.",
        concern_labels.len(),
        concern_labels.join("; ")
    )]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::{HIGH_RISK, LOW_RISK, SOME_CONCERNS};
    use super::*;
    use crate::appraisal::get_appraisal_definition;

    /// Every question answered "low risk", with the conditional questions the
    /// guidance does not ask set to not applicable.
    const LOW_ANSWERS: [(&str, &str); 22] = [
        (SEQUENCE_RANDOM, "yes"),
        (ALLOCATION_CONCEALED, "yes"),
        (BASELINE_IMBALANCE, "no"),
        (PARTICIPANTS_AWARE, "no"),
        (PERSONNEL_AWARE, "probably_no"),
        (TRIAL_CONTEXT_DEVIATIONS, "not_applicable"),
        (DEVIATIONS_AFFECTED_OUTCOME, "not_applicable"),
        (DEVIATIONS_BALANCED, "not_applicable"),
        (ANALYSIS_APPROPRIATE, "yes"),
        (SWITCHING_SUBSTANTIAL_IMPACT, "not_applicable"),
        (OUTCOME_DATA_COMPLETE, "yes"),
        (MISSING_DATA_NOT_BIASED, "not_applicable"),
        (MISSINGNESS_COULD_DEPEND, "not_applicable"),
        (MISSINGNESS_LIKELY_DEPENDS, "not_applicable"),
        (MEASUREMENT_INAPPROPRIATE, "no"),
        (MEASUREMENT_DIFFERED, "no"),
        (ASSESSORS_AWARE, "no"),
        (ASSESSMENT_COULD_BE_INFLUENCED, "not_applicable"),
        (ASSESSMENT_LIKELY_INFLUENCED, "not_applicable"),
        (ANALYSIS_PRESPECIFIED, "yes"),
        (OUTCOME_MEASUREMENT_SELECTED, "no"),
        (ANALYSIS_SELECTED, "no"),
    ];

    fn suggest_with(overrides: &[(&str, &str)]) -> JudgmentSuggestion {
        let definition = get_appraisal_definition("deepref-rct-rob2", 2).unwrap();
        let mut responses = serde_json::Map::new();
        for (id, value) in LOW_ANSWERS {
            responses.insert(id.to_owned(), json!(value));
        }
        for (id, value) in overrides {
            responses.insert((*id).to_owned(), json!(value));
        }
        suggest(&definition, &Value::Object(responses))
    }

    fn domain<'a>(suggestion: &'a JudgmentSuggestion, id: &str) -> &'a DomainJudgmentSuggestion {
        suggestion
            .domains
            .iter()
            .find(|domain| domain.domain_id == id)
            .expect("domain present")
    }

    /// Answer overrides, the expected judgment, and the expected drivers.
    type Row<'a> = (&'a [(&'a str, &'a str)], Option<&'a str>, &'a [&'a str]);

    /// Checks one domain's judgment and drivers for each row, so every path is named.
    fn assert_paths(domain_id: &str, rows: &[Row<'_>]) {
        for (overrides, judgment, drivers) in rows {
            let suggestion = suggest_with(overrides);
            let found = domain(&suggestion, domain_id);
            assert_eq!(
                found.judgment.as_deref(),
                *judgment,
                "{domain_id} judgment for {overrides:?}"
            );
            assert_eq!(
                found.drivers,
                drivers
                    .iter()
                    .map(|id| (*id).to_owned())
                    .collect::<Vec<_>>(),
                "{domain_id} drivers for {overrides:?}"
            );
        }
    }

    const LOW: Option<&str> = Some(LOW_RISK);
    const SOME: Option<&str> = Some(SOME_CONCERNS);
    const HIGH: Option<&str> = Some(HIGH_RISK);

    #[test]
    fn randomization_follows_figure_one() {
        assert_paths(
            RANDOMIZATION_PROCESS,
            &[
                // Concealment "no": high risk whatever the other answers.
                (
                    &[(ALLOCATION_CONCEALED, "no")],
                    HIGH,
                    &[ALLOCATION_CONCEALED],
                ),
                (
                    &[(ALLOCATION_CONCEALED, "probably_no")],
                    HIGH,
                    &[ALLOCATION_CONCEALED],
                ),
                // Concealed, sequence not random: some concerns.
                (&[(SEQUENCE_RANDOM, "no")], SOME, &[SEQUENCE_RANDOM]),
                (
                    &[(SEQUENCE_RANDOM, "probably_no")],
                    SOME,
                    &[SEQUENCE_RANDOM],
                ),
                // Concealed and random: the baseline decides.
                (&[(BASELINE_IMBALANCE, "yes")], SOME, &[BASELINE_IMBALANCE]),
                (
                    &[(BASELINE_IMBALANCE, "probably_yes")],
                    SOME,
                    &[BASELINE_IMBALANCE],
                ),
                (&[(BASELINE_IMBALANCE, "no")], LOW, &[]),
                (&[(BASELINE_IMBALANCE, "no_information")], LOW, &[]),
                // Sequence not reported is still low risk when concealed and balanced.
                (&[(SEQUENCE_RANDOM, "no_information")], LOW, &[]),
                (
                    &[
                        (SEQUENCE_RANDOM, "no_information"),
                        (BASELINE_IMBALANCE, "yes"),
                    ],
                    SOME,
                    &[BASELINE_IMBALANCE],
                ),
                // Concealment not reported: a baseline problem is high risk.
                (
                    &[
                        (ALLOCATION_CONCEALED, "no_information"),
                        (BASELINE_IMBALANCE, "yes"),
                    ],
                    HIGH,
                    &[ALLOCATION_CONCEALED, BASELINE_IMBALANCE],
                ),
                (
                    &[
                        (ALLOCATION_CONCEALED, "no_information"),
                        (BASELINE_IMBALANCE, "no"),
                    ],
                    SOME,
                    &[ALLOCATION_CONCEALED],
                ),
                (
                    &[
                        (ALLOCATION_CONCEALED, "no_information"),
                        (BASELINE_IMBALANCE, "no_information"),
                    ],
                    SOME,
                    &[ALLOCATION_CONCEALED],
                ),
            ],
        );
    }

    #[test]
    fn deviations_assignment_part_one_follows_table_six() {
        // Neither 2.1 nor 2.2 raises awareness: low, and 2.3 is not asked.
        assert_paths(
            DEVIATIONS_FROM_INTERVENTION,
            &[
                (&[], LOW, &[]),
                // Aware, but 2.3 says no deviations because of the trial context.
                (
                    &[
                        (PARTICIPANTS_AWARE, "yes"),
                        (TRIAL_CONTEXT_DEVIATIONS, "no"),
                    ],
                    LOW,
                    &[],
                ),
                // 2.3 no information: some concerns.
                (
                    &[
                        (PERSONNEL_AWARE, "no_information"),
                        (TRIAL_CONTEXT_DEVIATIONS, "no_information"),
                    ],
                    SOME,
                    &[PERSONNEL_AWARE, TRIAL_CONTEXT_DEVIATIONS],
                ),
                // 2.3 yes, 2.4 no: some concerns.
                (
                    &[
                        (PARTICIPANTS_AWARE, "yes"),
                        (TRIAL_CONTEXT_DEVIATIONS, "yes"),
                        (DEVIATIONS_AFFECTED_OUTCOME, "no"),
                        (DEVIATIONS_BALANCED, "not_applicable"),
                    ],
                    SOME,
                    &[PARTICIPANTS_AWARE, TRIAL_CONTEXT_DEVIATIONS],
                ),
                // 2.3 yes, 2.4 yes, 2.5 yes (balanced): some concerns.
                (
                    &[
                        (PARTICIPANTS_AWARE, "yes"),
                        (TRIAL_CONTEXT_DEVIATIONS, "probably_yes"),
                        (DEVIATIONS_AFFECTED_OUTCOME, "yes"),
                        (DEVIATIONS_BALANCED, "probably_yes"),
                    ],
                    SOME,
                    &[
                        PARTICIPANTS_AWARE,
                        TRIAL_CONTEXT_DEVIATIONS,
                        DEVIATIONS_AFFECTED_OUTCOME,
                    ],
                ),
                // 2.3 yes, 2.4 no information, 2.5 no: high risk.
                (
                    &[
                        (PARTICIPANTS_AWARE, "yes"),
                        (TRIAL_CONTEXT_DEVIATIONS, "yes"),
                        (DEVIATIONS_AFFECTED_OUTCOME, "no_information"),
                        (DEVIATIONS_BALANCED, "no"),
                    ],
                    HIGH,
                    &[
                        PARTICIPANTS_AWARE,
                        TRIAL_CONTEXT_DEVIATIONS,
                        DEVIATIONS_AFFECTED_OUTCOME,
                        DEVIATIONS_BALANCED,
                    ],
                ),
                // 2.3 yes, 2.4 yes, 2.5 no information: high risk.
                (
                    &[
                        (PARTICIPANTS_AWARE, "yes"),
                        (PERSONNEL_AWARE, "yes"),
                        (TRIAL_CONTEXT_DEVIATIONS, "yes"),
                        (DEVIATIONS_AFFECTED_OUTCOME, "yes"),
                        (DEVIATIONS_BALANCED, "no_information"),
                    ],
                    HIGH,
                    &[
                        PARTICIPANTS_AWARE,
                        PERSONNEL_AWARE,
                        TRIAL_CONTEXT_DEVIATIONS,
                        DEVIATIONS_AFFECTED_OUTCOME,
                        DEVIATIONS_BALANCED,
                    ],
                ),
            ],
        );
    }

    #[test]
    fn deviations_assignment_part_two_and_the_domain_combination() {
        assert_paths(
            DEVIATIONS_FROM_INTERVENTION,
            &[
                // Appropriate analysis: part two is low.
                (&[(ANALYSIS_APPROPRIATE, "probably_yes")], LOW, &[]),
                // Analysis not appropriate, switching not substantial: some concerns.
                (
                    &[
                        (ANALYSIS_APPROPRIATE, "no"),
                        (SWITCHING_SUBSTANTIAL_IMPACT, "probably_no"),
                    ],
                    SOME,
                    &[ANALYSIS_APPROPRIATE],
                ),
                // Analysis unknown, switching may be substantial: high risk.
                (
                    &[
                        (ANALYSIS_APPROPRIATE, "no_information"),
                        (SWITCHING_SUBSTANTIAL_IMPACT, "yes"),
                    ],
                    HIGH,
                    &[ANALYSIS_APPROPRIATE, SWITCHING_SUBSTANTIAL_IMPACT],
                ),
                (
                    &[
                        (ANALYSIS_APPROPRIATE, "probably_no"),
                        (SWITCHING_SUBSTANTIAL_IMPACT, "no_information"),
                    ],
                    HIGH,
                    &[ANALYSIS_APPROPRIATE, SWITCHING_SUBSTANTIAL_IMPACT],
                ),
                // Part one some concerns plus part two some concerns: both drive it.
                (
                    &[
                        (PARTICIPANTS_AWARE, "yes"),
                        (TRIAL_CONTEXT_DEVIATIONS, "no_information"),
                        (ANALYSIS_APPROPRIATE, "no"),
                        (SWITCHING_SUBSTANTIAL_IMPACT, "no"),
                    ],
                    SOME,
                    &[
                        PARTICIPANTS_AWARE,
                        TRIAL_CONTEXT_DEVIATIONS,
                        ANALYSIS_APPROPRIATE,
                    ],
                ),
                // Part one high risk outranks a part two concern.
                (
                    &[
                        (PARTICIPANTS_AWARE, "yes"),
                        (TRIAL_CONTEXT_DEVIATIONS, "yes"),
                        (DEVIATIONS_AFFECTED_OUTCOME, "yes"),
                        (DEVIATIONS_BALANCED, "no"),
                        (ANALYSIS_APPROPRIATE, "no"),
                        (SWITCHING_SUBSTANTIAL_IMPACT, "no"),
                    ],
                    HIGH,
                    &[
                        PARTICIPANTS_AWARE,
                        TRIAL_CONTEXT_DEVIATIONS,
                        DEVIATIONS_AFFECTED_OUTCOME,
                        DEVIATIONS_BALANCED,
                    ],
                ),
            ],
        );
    }

    #[test]
    fn missing_outcome_data_follows_table_ten() {
        assert_paths(
            MISSING_OUTCOME_DATA,
            &[
                (&[(OUTCOME_DATA_COMPLETE, "probably_yes")], LOW, &[]),
                (
                    &[
                        (OUTCOME_DATA_COMPLETE, "no"),
                        (MISSING_DATA_NOT_BIASED, "yes"),
                        (MISSINGNESS_COULD_DEPEND, "not_applicable"),
                        (MISSINGNESS_LIKELY_DEPENDS, "not_applicable"),
                    ],
                    LOW,
                    &[],
                ),
                (
                    &[
                        (OUTCOME_DATA_COMPLETE, "no_information"),
                        (MISSING_DATA_NOT_BIASED, "no"),
                        (MISSINGNESS_COULD_DEPEND, "probably_no"),
                        (MISSINGNESS_LIKELY_DEPENDS, "not_applicable"),
                    ],
                    LOW,
                    &[],
                ),
                (
                    &[
                        (OUTCOME_DATA_COMPLETE, "no"),
                        (MISSING_DATA_NOT_BIASED, "no"),
                        (MISSINGNESS_COULD_DEPEND, "no_information"),
                        (MISSINGNESS_LIKELY_DEPENDS, "no"),
                    ],
                    SOME,
                    &[
                        OUTCOME_DATA_COMPLETE,
                        MISSING_DATA_NOT_BIASED,
                        MISSINGNESS_COULD_DEPEND,
                    ],
                ),
                (
                    &[
                        (OUTCOME_DATA_COMPLETE, "no"),
                        (MISSING_DATA_NOT_BIASED, "probably_no"),
                        (MISSINGNESS_COULD_DEPEND, "yes"),
                        (MISSINGNESS_LIKELY_DEPENDS, "probably_yes"),
                    ],
                    HIGH,
                    &[
                        OUTCOME_DATA_COMPLETE,
                        MISSING_DATA_NOT_BIASED,
                        MISSINGNESS_COULD_DEPEND,
                        MISSINGNESS_LIKELY_DEPENDS,
                    ],
                ),
                (
                    &[
                        (OUTCOME_DATA_COMPLETE, "no"),
                        (MISSING_DATA_NOT_BIASED, "no"),
                        (MISSINGNESS_COULD_DEPEND, "no_information"),
                        (MISSINGNESS_LIKELY_DEPENDS, "no_information"),
                    ],
                    HIGH,
                    &[
                        OUTCOME_DATA_COMPLETE,
                        MISSING_DATA_NOT_BIASED,
                        MISSINGNESS_COULD_DEPEND,
                        MISSINGNESS_LIKELY_DEPENDS,
                    ],
                ),
            ],
        );
        // 3.2 has no "no information" option, so that answer leaves the domain unsuggested.
        let suggestion = suggest_with(&[
            (OUTCOME_DATA_COMPLETE, "no"),
            (MISSING_DATA_NOT_BIASED, "no_information"),
        ]);
        assert_eq!(domain(&suggestion, MISSING_OUTCOME_DATA).judgment, None);
    }

    #[test]
    fn outcome_measurement_follows_table_twelve() {
        assert_paths(
            OUTCOME_MEASUREMENT,
            &[
                (
                    &[(MEASUREMENT_INAPPROPRIATE, "yes")],
                    HIGH,
                    &[MEASUREMENT_INAPPROPRIATE],
                ),
                (
                    &[(MEASUREMENT_DIFFERED, "probably_yes")],
                    HIGH,
                    &[MEASUREMENT_DIFFERED],
                ),
                // Measurement the same in both groups and assessors blinded: low.
                (&[], LOW, &[]),
                (
                    &[
                        (ASSESSORS_AWARE, "yes"),
                        (ASSESSMENT_COULD_BE_INFLUENCED, "no"),
                    ],
                    LOW,
                    &[],
                ),
                // Assessors aware and could be influenced, not likely: some concerns.
                (
                    &[
                        (ASSESSORS_AWARE, "yes"),
                        (ASSESSMENT_COULD_BE_INFLUENCED, "yes"),
                        (ASSESSMENT_LIKELY_INFLUENCED, "no"),
                    ],
                    SOME,
                    &[ASSESSORS_AWARE, ASSESSMENT_COULD_BE_INFLUENCED],
                ),
                // Likely influenced: high risk.
                (
                    &[
                        (ASSESSORS_AWARE, "yes"),
                        (ASSESSMENT_COULD_BE_INFLUENCED, "yes"),
                        (ASSESSMENT_LIKELY_INFLUENCED, "yes"),
                    ],
                    HIGH,
                    &[
                        ASSESSORS_AWARE,
                        ASSESSMENT_COULD_BE_INFLUENCED,
                        ASSESSMENT_LIKELY_INFLUENCED,
                    ],
                ),
                // Measurement difference unknown: some concerns, or high risk if likely influenced.
                (
                    &[
                        (MEASUREMENT_DIFFERED, "no_information"),
                        (ASSESSORS_AWARE, "no"),
                    ],
                    SOME,
                    &[MEASUREMENT_DIFFERED],
                ),
                (
                    &[
                        (MEASUREMENT_DIFFERED, "no_information"),
                        (ASSESSORS_AWARE, "no_information"),
                        (ASSESSMENT_COULD_BE_INFLUENCED, "no"),
                    ],
                    SOME,
                    &[MEASUREMENT_DIFFERED, ASSESSORS_AWARE],
                ),
                (
                    &[
                        (MEASUREMENT_DIFFERED, "no_information"),
                        (ASSESSORS_AWARE, "yes"),
                        (ASSESSMENT_COULD_BE_INFLUENCED, "yes"),
                        (ASSESSMENT_LIKELY_INFLUENCED, "no"),
                    ],
                    SOME,
                    &[
                        MEASUREMENT_DIFFERED,
                        ASSESSORS_AWARE,
                        ASSESSMENT_COULD_BE_INFLUENCED,
                    ],
                ),
                (
                    &[
                        (MEASUREMENT_DIFFERED, "no_information"),
                        (ASSESSORS_AWARE, "yes"),
                        (ASSESSMENT_COULD_BE_INFLUENCED, "yes"),
                        (ASSESSMENT_LIKELY_INFLUENCED, "yes"),
                    ],
                    HIGH,
                    &[
                        MEASUREMENT_DIFFERED,
                        ASSESSORS_AWARE,
                        ASSESSMENT_COULD_BE_INFLUENCED,
                        ASSESSMENT_LIKELY_INFLUENCED,
                    ],
                ),
            ],
        );
    }

    #[test]
    fn reported_result_follows_table_fourteen() {
        assert_paths(
            REPORTED_RESULT,
            &[
                (&[], LOW, &[]),
                (
                    &[(ANALYSIS_PRESPECIFIED, "no")],
                    SOME,
                    &[ANALYSIS_PRESPECIFIED],
                ),
                (
                    &[(ANALYSIS_PRESPECIFIED, "no_information")],
                    SOME,
                    &[ANALYSIS_PRESPECIFIED],
                ),
                (
                    &[(ANALYSIS_SELECTED, "no_information")],
                    SOME,
                    &[ANALYSIS_SELECTED],
                ),
                (
                    &[(OUTCOME_MEASUREMENT_SELECTED, "no_information")],
                    SOME,
                    &[OUTCOME_MEASUREMENT_SELECTED],
                ),
                (
                    &[
                        (OUTCOME_MEASUREMENT_SELECTED, "no_information"),
                        (ANALYSIS_SELECTED, "no_information"),
                    ],
                    SOME,
                    &[OUTCOME_MEASUREMENT_SELECTED, ANALYSIS_SELECTED],
                ),
                (
                    &[(OUTCOME_MEASUREMENT_SELECTED, "yes")],
                    HIGH,
                    &[OUTCOME_MEASUREMENT_SELECTED],
                ),
                (
                    &[(ANALYSIS_SELECTED, "probably_yes")],
                    HIGH,
                    &[ANALYSIS_SELECTED],
                ),
                (
                    &[
                        (ANALYSIS_PRESPECIFIED, "no"),
                        (OUTCOME_MEASUREMENT_SELECTED, "yes"),
                        (ANALYSIS_SELECTED, "yes"),
                    ],
                    HIGH,
                    &[OUTCOME_MEASUREMENT_SELECTED, ANALYSIS_SELECTED],
                ),
            ],
        );
    }

    #[test]
    fn overall_judgment_follows_table_one() {
        let low = suggest_with(&[]);
        assert_eq!(low.overall_judgment.as_deref(), Some(LOW_RISK));
        assert!(low.reviewer_notes.is_empty());

        let one_concern = suggest_with(&[(SEQUENCE_RANDOM, "no")]);
        assert_eq!(one_concern.overall_judgment.as_deref(), Some(SOME_CONCERNS));
        assert!(one_concern.reviewer_notes.is_empty());

        let high = suggest_with(&[(ALLOCATION_CONCEALED, "no")]);
        assert_eq!(high.overall_judgment.as_deref(), Some(HIGH_RISK));
        assert!(high.reviewer_notes.is_empty());

        // Two domains with some concerns stay at some concerns, but the reviewer is asked to check.
        let several = suggest_with(&[
            (SEQUENCE_RANDOM, "no"),
            (ANALYSIS_SELECTED, "no_information"),
        ]);
        assert_eq!(several.overall_judgment.as_deref(), Some(SOME_CONCERNS));
        assert_eq!(several.reviewer_notes.len(), 1);
        assert!(several.reviewer_notes[0].starts_with("Some concerns in 2 domains"));

        // A high-risk domain outranks several concerns, so no reviewer note is needed.
        let high_and_some = suggest_with(&[
            (SEQUENCE_RANDOM, "no"),
            (ANALYSIS_SELECTED, "no_information"),
            (ALLOCATION_CONCEALED, "no"),
        ]);
        assert_eq!(high_and_some.overall_judgment.as_deref(), Some(HIGH_RISK));
        assert!(high_and_some.reviewer_notes.is_empty());
    }

    #[test]
    fn missing_or_misplaced_answers_leave_the_domain_unsuggested() {
        // A required answer is missing.
        let definition = get_appraisal_definition("deepref-rct-rob2", 2).unwrap();
        let mut responses = serde_json::Map::new();
        for (id, value) in LOW_ANSWERS {
            responses.insert(id.to_owned(), json!(value));
        }
        responses.remove(SEQUENCE_RANDOM);
        let suggestion = suggest(&definition, &Value::Object(responses.clone()));
        assert_eq!(domain(&suggestion, RANDOMIZATION_PROCESS).judgment, None);
        assert_eq!(suggestion.overall_judgment, None);

        // A conditional answer is "not applicable" although its condition holds.
        let misplaced = suggest_with(&[
            (PARTICIPANTS_AWARE, "yes"),
            (TRIAL_CONTEXT_DEVIATIONS, "not_applicable"),
        ]);
        assert_eq!(
            domain(&misplaced, DEVIATIONS_FROM_INTERVENTION).judgment,
            None
        );
    }
}
