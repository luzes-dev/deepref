use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{ExclusionReasonId, ProtocolVersionId, ReportId, ScreeningEventId};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreeningStatus {
    Unscreened,
    Included,
    Excluded,
    Maybe,
    AwaitingFullText,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "id")]
pub enum Actor {
    User(String),
    Automation(Uuid),
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreeningCommand {
    pub project_id: Uuid,
    pub report_id: ReportId,
    pub stage: ScreeningStage,
    pub decision: ScreeningDecision,
    pub exclusion_reason_id: Option<ExclusionReasonId>,
    pub protocol_version_id: ProtocolVersionId,
    pub actor: Actor,
    pub expected_revision: i64,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreeningEvent {
    pub id: ScreeningEventId,
    pub command: ScreeningCommand,
    pub supersedes_event_id: Option<ScreeningEventId>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreeningState {
    pub project_id: Uuid,
    pub report_id: ReportId,
    pub title_abstract_status: ScreeningStatus,
    pub full_text_status: ScreeningStatus,
    pub final_status: ScreeningStatus,
    pub revision: i64,
    pub last_event_id: Option<ScreeningEventId>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ScreeningError {
    #[error("full-text exclusion requires exactly one primary exclusion reason")]
    MissingFullTextExclusionReason,
    #[error("an exclusion reason is only valid for an exclusion decision")]
    ExclusionReasonOnNonExclusion,
}

impl ScreeningCommand {
    pub fn validate(&self) -> Result<(), ScreeningError> {
        if self.stage == ScreeningStage::FullText
            && self.decision == ScreeningDecision::Exclude
            && self.exclusion_reason_id.is_none()
        {
            return Err(ScreeningError::MissingFullTextExclusionReason);
        }

        if self.decision != ScreeningDecision::Exclude && self.exclusion_reason_id.is_some() {
            return Err(ScreeningError::ExclusionReasonOnNonExclusion);
        }

        Ok(())
    }
}

impl ScreeningState {
    pub fn initial(project_id: Uuid, report_id: ReportId, now: DateTime<Utc>) -> Self {
        Self {
            project_id,
            report_id,
            title_abstract_status: ScreeningStatus::Unscreened,
            full_text_status: ScreeningStatus::Unscreened,
            final_status: ScreeningStatus::Unscreened,
            revision: 0,
            last_event_id: None,
            updated_at: now,
        }
    }

    #[must_use]
    pub fn status_for(command: &ScreeningCommand) -> ScreeningStatus {
        match (command.stage, command.decision) {
            (ScreeningStage::TitleAbstract, ScreeningDecision::Include) => {
                ScreeningStatus::AwaitingFullText
            }
            (_, ScreeningDecision::Include) => ScreeningStatus::Included,
            (_, ScreeningDecision::Exclude) => ScreeningStatus::Excluded,
            (_, ScreeningDecision::Maybe) => ScreeningStatus::Maybe,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(stage: ScreeningStage, decision: ScreeningDecision) -> ScreeningCommand {
        ScreeningCommand {
            project_id: Uuid::new_v4(),
            report_id: ReportId::new(),
            stage,
            decision,
            exclusion_reason_id: None,
            protocol_version_id: ProtocolVersionId::new(),
            actor: Actor::User("reviewer".to_owned()),
            expected_revision: 0,
            notes: None,
        }
    }

    #[test]
    fn full_text_exclusion_requires_reason() {
        let value = command(ScreeningStage::FullText, ScreeningDecision::Exclude);
        assert_eq!(
            value.validate(),
            Err(ScreeningError::MissingFullTextExclusionReason)
        );
    }

    #[test]
    fn maybe_remains_distinct_from_include() {
        let maybe = command(ScreeningStage::TitleAbstract, ScreeningDecision::Maybe);
        let include = command(ScreeningStage::TitleAbstract, ScreeningDecision::Include);
        assert_ne!(
            ScreeningState::status_for(&maybe),
            ScreeningState::status_for(&include)
        );
    }
}
