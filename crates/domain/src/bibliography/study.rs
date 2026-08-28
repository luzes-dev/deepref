use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{ReportId, Study, StudyId};
use crate::Actor;

const MAX_STUDY_TITLE_LENGTH: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StudyTitleError {
    #[error("study title must not be empty")]
    Blank,
    #[error("study title must be at most {MAX_STUDY_TITLE_LENGTH} characters")]
    TooLong,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StudyTitle(String);

impl StudyTitle {
    pub fn new(value: impl Into<String>) -> Result<Self, StudyTitleError> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(StudyTitleError::Blank);
        }
        if trimmed.chars().count() > MAX_STUDY_TITLE_LENGTH {
            return Err(StudyTitleError::TooLong);
        }
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StudyDesign {
    Rct,
    NonRandomizedIntervention,
    Cohort,
    CaseControl,
    CrossSectional,
    DiagnosticAccuracy,
    PredictionModel,
    Qualitative,
    SystematicReview,
    CaseSeries,
}

impl StudyDesign {
    pub const ALL: [Self; 10] = [
        Self::Rct,
        Self::NonRandomizedIntervention,
        Self::Cohort,
        Self::CaseControl,
        Self::CrossSectional,
        Self::DiagnosticAccuracy,
        Self::PredictionModel,
        Self::Qualitative,
        Self::SystematicReview,
        Self::CaseSeries,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rct => "rct",
            Self::NonRandomizedIntervention => "non_randomized_intervention",
            Self::Cohort => "cohort",
            Self::CaseControl => "case_control",
            Self::CrossSectional => "cross_sectional",
            Self::DiagnosticAccuracy => "diagnostic_accuracy",
            Self::PredictionModel => "prediction_model",
            Self::Qualitative => "qualitative",
            Self::SystematicReview => "systematic_review",
            Self::CaseSeries => "case_series",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Rct => "Randomized controlled trial",
            Self::NonRandomizedIntervention => "Non-randomized intervention",
            Self::Cohort => "Cohort",
            Self::CaseControl => "Case-control",
            Self::CrossSectional => "Cross-sectional",
            Self::DiagnosticAccuracy => "Diagnostic accuracy",
            Self::PredictionModel => "Prediction model",
            Self::Qualitative => "Qualitative",
            Self::SystematicReview => "Systematic review",
            Self::CaseSeries => "Case series",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|design| design.as_str() == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StudyReportRole {
    ReportOfStudy,
    Protocol,
    PrimaryOutcome,
    SafetyAnalysis,
    EconomicAnalysis,
    FollowUp,
}

impl StudyReportRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReportOfStudy => "report_of_study",
            Self::Protocol => "protocol",
            Self::PrimaryOutcome => "primary_outcome",
            Self::SafetyAnalysis => "safety_analysis",
            Self::EconomicAnalysis => "economic_analysis",
            Self::FollowUp => "follow_up",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "report_of_study" => Some(Self::ReportOfStudy),
            "protocol" => Some(Self::Protocol),
            "primary_outcome" => Some(Self::PrimaryOutcome),
            "safety_analysis" => Some(Self::SafetyAnalysis),
            "economic_analysis" => Some(Self::EconomicAnalysis),
            "follow_up" => Some(Self::FollowUp),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StudyDesignContext {
    pub physiotherapy: bool,
    pub exposure: bool,
    pub prediction_or_ai: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppraisalToolSuggestion {
    pub tool: String,
    pub rationale: String,
}

pub fn suggest_appraisal_tools(
    design: StudyDesign,
    context: StudyDesignContext,
) -> Vec<AppraisalToolSuggestion> {
    let mut suggestions = match design {
        StudyDesign::Rct => vec![suggestion("RoB 2", "randomized intervention bias")],
        StudyDesign::NonRandomizedIntervention => {
            vec![suggestion("ROBINS-I", "non-randomized intervention bias")]
        }
        StudyDesign::Cohort | StudyDesign::CaseControl | StudyDesign::CrossSectional => Vec::new(),
        StudyDesign::DiagnosticAccuracy => vec![suggestion("QUADAS", "diagnostic accuracy bias")],
        StudyDesign::PredictionModel => vec![suggestion(
            "PROBAST",
            if context.prediction_or_ai {
                "prediction or AI model bias"
            } else {
                "prediction model bias"
            },
        )],
        StudyDesign::Qualitative => vec![
            suggestion("JBI", "qualitative study appraisal"),
            suggestion("CASP", "qualitative study appraisal"),
        ],
        StudyDesign::SystematicReview => vec![
            suggestion("AMSTAR 2", "systematic review methodological quality"),
            suggestion("ROBIS", "systematic review risk of bias"),
        ],
        StudyDesign::CaseSeries => Vec::new(),
    };

    if context.physiotherapy && design == StudyDesign::Rct {
        suggestions.push(suggestion(
            "PEDro",
            "physiotherapy trial reporting and quality",
        ));
    }
    if context.exposure
        && matches!(
            design,
            StudyDesign::Cohort
                | StudyDesign::CaseControl
                | StudyDesign::CrossSectional
                | StudyDesign::NonRandomizedIntervention
        )
    {
        suggestions.push(suggestion("ROBINS-E", "exposure effects bias"));
        suggestions.push(suggestion("JBI", "observational study appraisal"));
    }
    suggestions
}

fn suggestion(tool: &str, rationale: &str) -> AppraisalToolSuggestion {
    AppraisalToolSuggestion {
        tool: tool.to_owned(),
        rationale: rationale.to_owned(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum StudyMembershipError {
    #[error("report is already assigned to this study")]
    AlreadyMember,
    #[error("report is not assigned to this study")]
    NotMember,
    #[error("study revision does not match the expected revision")]
    RevisionConflict { expected: u64, actual: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum StudyRevisionError {
    #[error("study revision does not match the expected revision")]
    Conflict { expected: u64, actual: u64 },
}

impl Study {
    pub fn new(id: StudyId, title: StudyTitle) -> Self {
        Self {
            id,
            title,
            design: None,
            revision: 0,
            report_ids: Vec::new(),
        }
    }

    pub fn rename(
        &mut self,
        title: StudyTitle,
        expected_revision: u64,
    ) -> Result<(), StudyRevisionError> {
        self.check_revision(expected_revision)?;
        self.title = title;
        self.revision += 1;
        Ok(())
    }

    pub fn classify(
        &mut self,
        design: StudyDesign,
        expected_revision: u64,
    ) -> Result<(), StudyRevisionError> {
        self.check_revision(expected_revision)?;
        self.design = Some(design);
        self.revision += 1;
        Ok(())
    }

    pub fn assign_report(
        &mut self,
        report_id: ReportId,
        expected_revision: u64,
    ) -> Result<StudyMembershipChange, StudyMembershipError> {
        if self.revision != expected_revision {
            return Err(StudyMembershipError::RevisionConflict {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        if self.report_ids.contains(&report_id) {
            return Err(StudyMembershipError::AlreadyMember);
        }
        self.report_ids.push(report_id);
        self.revision += 1;
        Ok(StudyMembershipChange::Assigned { report_id })
    }

    pub fn remove_report(
        &mut self,
        report_id: ReportId,
        expected_revision: u64,
    ) -> Result<StudyMembershipChange, StudyMembershipError> {
        if self.revision != expected_revision {
            return Err(StudyMembershipError::RevisionConflict {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        let Some(position) = self.report_ids.iter().position(|id| *id == report_id) else {
            return Err(StudyMembershipError::NotMember);
        };
        self.report_ids.remove(position);
        self.revision += 1;
        Ok(StudyMembershipChange::Removed { report_id })
    }

    fn check_revision(&self, expected_revision: u64) -> Result<(), StudyRevisionError> {
        if self.revision == expected_revision {
            Ok(())
        } else {
            Err(StudyRevisionError::Conflict {
                expected: expected_revision,
                actual: self.revision,
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum StudyMembershipChange {
    Assigned { report_id: ReportId },
    Removed { report_id: ReportId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StudyCreated {
    pub study_id: StudyId,
    pub title: StudyTitle,
    pub actor: Actor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportAssignedToStudy {
    pub study_id: StudyId,
    pub report_id: ReportId,
    pub previous_study_id: Option<StudyId>,
    pub role: StudyReportRole,
    pub before_revision: u64,
    pub result_revision: u64,
    pub actor: Actor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StudyRenamed {
    pub study_id: StudyId,
    pub title: StudyTitle,
    pub before_revision: u64,
    pub result_revision: u64,
    pub actor: Actor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportRemovedFromStudy {
    pub study_id: StudyId,
    pub report_id: ReportId,
    pub result_revision: u64,
    pub actor: Actor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StudyClassified {
    pub study_id: StudyId,
    pub previous_design: Option<StudyDesign>,
    pub design: StudyDesign,
    pub context: StudyDesignContext,
    pub before_revision: u64,
    pub result_revision: u64,
    pub actor: Actor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "payload")]
pub enum StudyEvent {
    StudyCreated(StudyCreated),
    StudyRenamed(StudyRenamed),
    ReportAssignedToStudy(ReportAssignedToStudy),
    ReportRemovedFromStudy(ReportRemovedFromStudy),
    StudyClassified(StudyClassified),
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn normalized_catalog_is_closed_and_stable() {
        assert_eq!(StudyDesign::ALL.len(), 10);
        assert_eq!(StudyDesign::parse("rct"), Some(StudyDesign::Rct));
        assert!(StudyDesign::parse("randomized_trial").is_none());
    }

    #[test]
    fn suggestions_are_deterministic_and_contextual() {
        let default = suggest_appraisal_tools(StudyDesign::Rct, StudyDesignContext::default());
        assert_eq!(
            default
                .iter()
                .map(|item| item.tool.as_str())
                .collect::<Vec<_>>(),
            ["RoB 2"]
        );
        let physiotherapy = suggest_appraisal_tools(
            StudyDesign::Rct,
            StudyDesignContext {
                physiotherapy: true,
                ..StudyDesignContext::default()
            },
        );
        assert_eq!(
            physiotherapy
                .iter()
                .map(|item| item.tool.as_str())
                .collect::<Vec<_>>(),
            ["RoB 2", "PEDro"]
        );
    }

    #[test]
    fn study_title_is_trimmed_and_bounded() {
        assert_eq!(StudyTitle::new("  trial  ").unwrap().as_str(), "trial");
        assert_eq!(StudyTitle::new("  ").unwrap_err(), StudyTitleError::Blank);
        assert_eq!(
            StudyTitle::new("x".repeat(MAX_STUDY_TITLE_LENGTH + 1)).unwrap_err(),
            StudyTitleError::TooLong
        );
        let _ = StudyId::from(Uuid::new_v4());
    }

    #[test]
    fn study_aggregate_keeps_membership_and_revision_transitions_explicit() {
        let report_id: ReportId = Uuid::new_v4().into();
        let mut study = Study::new(Uuid::new_v4().into(), StudyTitle::new("Trial").unwrap());
        study.assign_report(report_id, 0).unwrap();
        assert_eq!(study.revision, 1);
        study.remove_report(report_id, 1).unwrap();
        assert!(study.report_ids.is_empty());
        assert_eq!(study.revision, 2);
    }
}
