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

/// Persisted lifecycle of an AI-first cohort.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiFirstCohortStatus {
    Open,
    Closed,
    Auditing,
    Passed,
    Failed,
    Invalidated,
    Declined,
    Finalized,
}

impl AiFirstCohortStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Auditing => "auditing",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Invalidated => "invalidated",
            Self::Declined => "declined",
            Self::Finalized => "finalized",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "closed" => Some(Self::Closed),
            "auditing" => Some(Self::Auditing),
            "passed" => Some(Self::Passed),
            "failed" => Some(Self::Failed),
            "invalidated" => Some(Self::Invalidated),
            "declined" => Some(Self::Declined),
            "finalized" => Some(Self::Finalized),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiFirstCeiling {
    Off,
    Routing,
    CohortFinalization,
}

impl AiFirstCeiling {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Routing => "routing",
            Self::CohortFinalization => "cohort_finalization",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "routing" => Some(Self::Routing),
            "cohort_finalization" => Some(Self::CohortFinalization),
            _ => None,
        }
    }
}
