use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

macro_rules! typed_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub const fn new(value: Uuid) -> Self {
                Self(value)
            }

            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self::new(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.as_uuid()
            }
        }
    };
}

typed_id!(ReportId);
typed_id!(StudyId);
typed_id!(ProjectId);
typed_id!(ProtocolVersionId);
typed_id!(ExclusionReasonId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreeningStage {
    TitleAbstract,
    FullText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreeningDecision {
    Include,
    Exclude,
    Maybe,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ScreeningValidationError {
    #[error("full-text exclusion requires an exclusion reason")]
    FullTextExclusionRequiresReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenReportCommand {
    pub project_id: ProjectId,
    pub report_id: ReportId,
    pub stage: ScreeningStage,
    pub decision: ScreeningDecision,
    pub exclusion_reason_id: Option<ExclusionReasonId>,
    pub protocol_version_id: ProtocolVersionId,
    pub expected_revision: i64,
}

impl ScreenReportCommand {
    pub fn validate(&self) -> Result<(), ScreeningValidationError> {
        if matches!(self.stage, ScreeningStage::FullText)
            && matches!(self.decision, ScreeningDecision::Exclude)
            && self.exclusion_reason_id.is_none()
        {
            return Err(ScreeningValidationError::FullTextExclusionRequiresReason);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(stage: ScreeningStage, decision: ScreeningDecision) -> ScreenReportCommand {
        ScreenReportCommand {
            project_id: Uuid::new_v4().into(),
            report_id: Uuid::new_v4().into(),
            stage,
            decision,
            exclusion_reason_id: None,
            protocol_version_id: Uuid::new_v4().into(),
            expected_revision: 0,
        }
    }

    #[test]
    fn full_text_exclusion_requires_a_reason() {
        assert_eq!(
            command(ScreeningStage::FullText, ScreeningDecision::Exclude).validate(),
            Err(ScreeningValidationError::FullTextExclusionRequiresReason)
        );
    }

    #[test]
    fn title_abstract_exclusion_can_be_reasonless() {
        assert!(
            command(ScreeningStage::TitleAbstract, ScreeningDecision::Exclude)
                .validate()
                .is_ok()
        );
    }
}
