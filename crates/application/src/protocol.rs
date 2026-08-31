use std::collections::BTreeMap;

use deepref_domain::{
    CriterionDimension, CriterionKind, CriterionStage, FrameworkKind, ProjectId, ProtocolFramework,
    ProtocolValidationError,
};
use thiserror::Error;
use uuid::Uuid;

pub const MAX_PROTOCOL_NAME_LENGTH: usize = 200;
pub const MAX_PROTOCOL_TEXT_LENGTH: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolCriterionCommand {
    pub id: Option<Uuid>,
    pub kind: CriterionKind,
    pub stage: CriterionStage,
    pub dimension: CriterionDimension,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveProtocolDraftCommand {
    pub project_id: ProjectId,
    pub protocol_version_id: Option<Uuid>,
    pub name: String,
    pub objective: String,
    pub question: String,
    pub framework_kind: FrameworkKind,
    pub framework_fields: BTreeMap<String, String>,
    pub criteria: Vec<ProtocolCriterionCommand>,
    pub expected_revision: i64,
}

impl SaveProtocolDraftCommand {
    pub fn validate(&self) -> Result<(), ProtocolCommandError> {
        validate_protocol_text(&self.name, MAX_PROTOCOL_NAME_LENGTH, "name")?;
        validate_protocol_text(&self.objective, MAX_PROTOCOL_TEXT_LENGTH, "objective")?;
        validate_protocol_text(&self.question, MAX_PROTOCOL_TEXT_LENGTH, "question")?;
        ProtocolFramework::new(self.framework_kind, self.framework_fields.clone())?;
        if self.expected_revision < 0 {
            return Err(ProtocolCommandError::InvalidRevision);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishProtocolCommand {
    pub project_id: ProjectId,
    pub protocol_version_id: Uuid,
    pub expected_revision: i64,
}

impl PublishProtocolCommand {
    pub fn validate(&self) -> Result<(), ProtocolCommandError> {
        if self.expected_revision < 1 {
            return Err(ProtocolCommandError::InvalidRevision);
        }
        Ok(())
    }
}

pub fn validate_protocol_text(
    value: &str,
    max_length: usize,
    field: &'static str,
) -> Result<(), ProtocolCommandError> {
    if value.trim().is_empty() {
        return Err(ProtocolCommandError::BlankField(field));
    }
    if value.chars().count() > max_length {
        return Err(ProtocolCommandError::FieldTooLong(field));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProtocolCommandError {
    #[error("protocol {0} must not be blank")]
    BlankField(&'static str),
    #[error("protocol {0} is too long")]
    FieldTooLong(&'static str),
    #[error("protocol revision must not be negative")]
    InvalidRevision,
    #[error(transparent)]
    Validation(#[from] ProtocolValidationError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_commands_reject_blank_protocol_text() {
        let result = validate_protocol_text("  ", MAX_PROTOCOL_NAME_LENGTH, "name");
        assert_eq!(result, Err(ProtocolCommandError::BlankField("name")));
    }

    #[test]
    fn draft_commands_validate_known_framework_fields() {
        let command = SaveProtocolDraftCommand {
            project_id: ProjectId::from(Uuid::new_v4()),
            protocol_version_id: None,
            name: "Protocol".to_owned(),
            objective: "Objective".to_owned(),
            question: "Question?".to_owned(),
            framework_kind: FrameworkKind::Pico,
            framework_fields: BTreeMap::from([("population".to_owned(), "People".to_owned())]),
            criteria: Vec::new(),
            expected_revision: 0,
        };

        assert!(matches!(
            command.validate(),
            Err(ProtocolCommandError::Validation(
                ProtocolValidationError::MissingFrameworkField { .. }
            ))
        ));
    }
}
