//! Conservative deterministic routing policy for validated title/abstract runs.
use deepref_ai::{CriterionResult, ScreeningAnalysis, ScreeningInput, SuggestedDecision};
use deepref_domain::{CriterionKind, EligibilityCriterion};

pub const AI_FIRST_POLICY_VERSION: u32 = 1;

/// Inputs have already passed the production schema and grounding validator.
/// Both independent screens must support a grounded mechanical exclusion.
pub fn automation_eligible_exclusion(
    input: &ScreeningInput,
    criteria: &[EligibilityCriterion],
    primary: &ScreeningAnalysis,
    independent: &ScreeningAnalysis,
) -> bool {
    if input.stage != deepref_ai::ScreeningStage::TitleAbstract
        || input.title.as_deref().is_none_or(|s| s.trim().is_empty())
        || input
            .abstract_text
            .as_deref()
            .is_none_or(|s| s.trim().chars().count() < 50)
    {
        return false;
    }
    [primary, independent].into_iter().all(|analysis| {
        analysis.report_id == input.report_id.as_uuid()
            && analysis.stage == input.stage
            && analysis.expected_revision == input.expected_revision
            && analysis.protocol_version_id == input.protocol_version_id.as_uuid()
            && matches!(
                analysis.suggested_decision,
                SuggestedDecision::Exclude {
                    exclusion_reason_id: None
                }
            )
            && analysis.uncertainties.is_empty()
            && {
                let decisive: Vec<_> = analysis
                    .criteria
                    .iter()
                    .filter(|judgment| {
                        criteria.iter().any(|criterion| {
                            criterion.id == judgment.criterion_id
                                && matches!(
                                    (criterion.kind, judgment.judgment),
                                    (CriterionKind::Inclusion, CriterionResult::DoesNotMeet)
                                        | (CriterionKind::Exclusion, CriterionResult::Meets)
                                )
                        })
                    })
                    .collect();
                !decisive.is_empty()
                    && decisive.iter().all(|judgment| {
                        !judgment.evidence.is_empty()
                            && judgment.evidence.iter().all(|evidence| {
                                matches!(evidence,
                            deepref_ai::ScreeningEvidence::ReportMetadata { report_id, .. }
                            if *report_id == input.report_id.as_uuid())
                            })
                    })
            }
    })
}
