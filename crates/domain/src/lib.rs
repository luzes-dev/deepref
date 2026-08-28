use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub mod acquisition;
pub mod bibliography;
pub mod documents;
pub mod protocol;

pub use acquisition::{AcquisitionRunId, AcquisitionSource, AcquisitionStatus, ImportFormat};
pub use bibliography::{
    AppraisalToolSuggestion, Citation, DoiError, IdentifierError, IdentifierScheme, Record,
    RecordId, Report, ReportAssignedToStudy, ReportId, ReportIdentifier, ReportRemovedFromStudy,
    Study, StudyClassified, StudyCreated, StudyDesign, StudyDesignContext, StudyEvent, StudyId,
    StudyMembershipChange, StudyMembershipError, StudyRenamed, StudyReportRole, StudyRevisionError,
    StudyTitle, StudyTitleError, Title, TitleError, normalize_bibliography_title, normalize_doi,
    suggest_appraisal_tools,
};
pub use documents::{
    DocumentBlock, DocumentBlockId, DocumentContent, DocumentId, DocumentMetadata, DocumentSource,
    DocumentStatus, DocumentStatusTransitionError, NormalizedBoundingBox, OcrRequirement,
};
pub use protocol::{
    CriterionDimension, CriterionKind, CriterionStage, EligibilityCriterion, FrameworkKind,
    ProtocolFramework, ProtocolStatus, ProtocolValidationError, validate_criteria,
};

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

typed_id!(ProjectId);
typed_id!(ProtocolVersionId);
typed_id!(ExclusionReasonId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    User,
    Automation,
    System,
}

impl ActorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Automation => "automation",
            Self::System => "system",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "automation" => Some(Self::Automation),
            "system" => Some(Self::System),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ActorValidationError {
    #[error("actor id must not be blank")]
    BlankId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    kind: ActorKind,
    id: String,
}

impl Actor {
    pub fn new(kind: ActorKind, id: impl Into<String>) -> Result<Self, ActorValidationError> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(ActorValidationError::BlankId);
        }
        Ok(Self { kind, id })
    }

    pub const fn kind(&self) -> ActorKind {
        self.kind
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

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
    #[error("title/abstract decisions cannot carry an exclusion reason")]
    TitleAbstractReasonNotAllowed,
    #[error("full-text screening requires a title/abstract Include decision")]
    FullTextRequiresTitleAbstractInclude,
    #[error("full-text exclusion requires exactly one exclusion reason")]
    FullTextExclusionRequiresReason,
    #[error("non-exclude full-text decisions cannot carry an exclusion reason")]
    FullTextNonExclusionReasonNotAllowed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CurrentScreeningState {
    pub title_abstract: Option<ScreeningDecision>,
    pub full_text: Option<ScreeningDecision>,
    pub full_text_exclusion_reason_id: Option<ExclusionReasonId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreeningTransition {
    Applied(CurrentScreeningState),
    Repeated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ScreeningUndoValidationError {
    #[error("the restored screening state is not valid")]
    InvalidRestoredState,
    #[error("full-text undo cannot alter the title/abstract decision")]
    FullTextCannotAlterTitleAbstract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenReportTransitionCommand {
    pub stage: ScreeningStage,
    pub decision: ScreeningDecision,
    pub exclusion_reason_id: Option<ExclusionReasonId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UndoScreeningTransitionCommand {
    pub stage: ScreeningStage,
}

pub fn transition(
    command: &ScreenReportTransitionCommand,
    current: CurrentScreeningState,
) -> Result<ScreeningTransition, ScreeningValidationError> {
    match command.stage {
        ScreeningStage::TitleAbstract => {
            if command.exclusion_reason_id.is_some() {
                return Err(ScreeningValidationError::TitleAbstractReasonNotAllowed);
            }
            if current.title_abstract == Some(command.decision) {
                return Ok(ScreeningTransition::Repeated);
            }
            Ok(ScreeningTransition::Applied(CurrentScreeningState {
                title_abstract: Some(command.decision),
                full_text: if command.decision == ScreeningDecision::Include {
                    current.full_text
                } else {
                    None
                },
                full_text_exclusion_reason_id: if command.decision == ScreeningDecision::Include {
                    current.full_text_exclusion_reason_id
                } else {
                    None
                },
            }))
        }
        ScreeningStage::FullText => {
            if current.title_abstract != Some(ScreeningDecision::Include) {
                return Err(ScreeningValidationError::FullTextRequiresTitleAbstractInclude);
            }
            match command.decision {
                ScreeningDecision::Exclude if command.exclusion_reason_id.is_none() => {
                    Err(ScreeningValidationError::FullTextExclusionRequiresReason)
                }
                ScreeningDecision::Include | ScreeningDecision::Maybe
                    if command.exclusion_reason_id.is_some() =>
                {
                    Err(ScreeningValidationError::FullTextNonExclusionReasonNotAllowed)
                }
                _ => {
                    if current.full_text == Some(command.decision)
                        && current.full_text_exclusion_reason_id == command.exclusion_reason_id
                    {
                        return Ok(ScreeningTransition::Repeated);
                    }
                    Ok(ScreeningTransition::Applied(CurrentScreeningState {
                        full_text: Some(command.decision),
                        full_text_exclusion_reason_id: command.exclusion_reason_id,
                        ..current
                    }))
                }
            }
        }
    }
}

pub fn undo_transition(
    command: &UndoScreeningTransitionCommand,
    current: CurrentScreeningState,
    restored: CurrentScreeningState,
) -> Result<ScreeningTransition, ScreeningUndoValidationError> {
    match command.stage {
        ScreeningStage::TitleAbstract => {
            let mut restored = restored;
            // A title/abstract exclusion or Maybe makes dependent full-text
            // state inapplicable. Undoing that decision is allowed to clear it.
            if restored.title_abstract != Some(ScreeningDecision::Include) {
                restored.full_text = None;
                restored.full_text_exclusion_reason_id = None;
            }
            validate_screening_state(restored)?;
            if current == restored {
                return Ok(ScreeningTransition::Repeated);
            }
            Ok(ScreeningTransition::Applied(restored))
        }
        ScreeningStage::FullText => {
            if current.title_abstract != restored.title_abstract {
                return Err(ScreeningUndoValidationError::FullTextCannotAlterTitleAbstract);
            }
            validate_screening_state(restored)?;
            if current == restored {
                return Ok(ScreeningTransition::Repeated);
            }
            Ok(ScreeningTransition::Applied(restored))
        }
    }
}

fn validate_screening_state(
    state: CurrentScreeningState,
) -> Result<(), ScreeningUndoValidationError> {
    if state.title_abstract != Some(ScreeningDecision::Include)
        && (state.full_text.is_some() || state.full_text_exclusion_reason_id.is_some())
    {
        return Err(ScreeningUndoValidationError::InvalidRestoredState);
    }
    match (state.full_text, state.full_text_exclusion_reason_id) {
        (Some(ScreeningDecision::Exclude), None)
        | (Some(ScreeningDecision::Include | ScreeningDecision::Maybe), Some(_))
        | (None, Some(_)) => Err(ScreeningUndoValidationError::InvalidRestoredState),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(
        stage: ScreeningStage,
        decision: ScreeningDecision,
        exclusion_reason_id: Option<ExclusionReasonId>,
    ) -> ScreenReportTransitionCommand {
        ScreenReportTransitionCommand {
            stage,
            decision,
            exclusion_reason_id,
        }
    }

    #[test]
    fn full_text_exclusion_requires_a_reason() {
        assert_eq!(
            transition(
                &command(ScreeningStage::FullText, ScreeningDecision::Exclude, None),
                CurrentScreeningState {
                    title_abstract: Some(ScreeningDecision::Include),
                    ..CurrentScreeningState::default()
                }
            ),
            Err(ScreeningValidationError::FullTextExclusionRequiresReason)
        );
    }

    #[test]
    fn title_abstract_decisions_cannot_carry_a_reason() {
        assert!(matches!(
            transition(
                &command(
                    ScreeningStage::TitleAbstract,
                    ScreeningDecision::Exclude,
                    Some(Uuid::new_v4().into()),
                ),
                CurrentScreeningState::default(),
            ),
            Err(ScreeningValidationError::TitleAbstractReasonNotAllowed)
        ));
    }

    #[test]
    fn full_text_requires_title_abstract_include() {
        assert_eq!(
            transition(
                &command(ScreeningStage::FullText, ScreeningDecision::Maybe, None),
                CurrentScreeningState::default(),
            ),
            Err(ScreeningValidationError::FullTextRequiresTitleAbstractInclude)
        );
    }

    #[test]
    fn non_exclude_full_text_decisions_cannot_carry_a_reason() {
        assert_eq!(
            transition(
                &command(
                    ScreeningStage::FullText,
                    ScreeningDecision::Include,
                    Some(Uuid::new_v4().into()),
                ),
                CurrentScreeningState {
                    title_abstract: Some(ScreeningDecision::Include),
                    ..CurrentScreeningState::default()
                },
            ),
            Err(ScreeningValidationError::FullTextNonExclusionReasonNotAllowed)
        );
    }

    #[test]
    fn maybe_is_distinct_and_repeated_decisions_are_typed() {
        let current = CurrentScreeningState::default();
        let maybe = transition(
            &command(
                ScreeningStage::TitleAbstract,
                ScreeningDecision::Maybe,
                None,
            ),
            current,
        )
        .expect("Maybe should be a valid first decision");
        assert_eq!(
            maybe,
            ScreeningTransition::Applied(CurrentScreeningState {
                title_abstract: Some(ScreeningDecision::Maybe),
                ..current
            })
        );
        assert_eq!(
            transition(
                &command(
                    ScreeningStage::TitleAbstract,
                    ScreeningDecision::Maybe,
                    None,
                ),
                CurrentScreeningState {
                    title_abstract: Some(ScreeningDecision::Maybe),
                    ..current
                },
            ),
            Ok(ScreeningTransition::Repeated)
        );
    }

    #[test]
    fn repeated_full_text_exclusion_includes_its_reason() {
        let reason = ExclusionReasonId::from(Uuid::new_v4());
        assert_eq!(
            transition(
                &command(
                    ScreeningStage::FullText,
                    ScreeningDecision::Exclude,
                    Some(reason),
                ),
                CurrentScreeningState {
                    title_abstract: Some(ScreeningDecision::Include),
                    full_text: Some(ScreeningDecision::Exclude),
                    full_text_exclusion_reason_id: Some(reason),
                },
            ),
            Ok(ScreeningTransition::Repeated)
        );
    }

    #[test]
    fn title_abstract_change_away_from_include_clears_full_text_state() {
        let reason = ExclusionReasonId::from(Uuid::new_v4());
        let next = transition(
            &command(
                ScreeningStage::TitleAbstract,
                ScreeningDecision::Maybe,
                None,
            ),
            CurrentScreeningState {
                title_abstract: Some(ScreeningDecision::Include),
                full_text: Some(ScreeningDecision::Exclude),
                full_text_exclusion_reason_id: Some(reason),
            },
        )
        .expect("changing the title/abstract decision should be valid");

        assert_eq!(
            next,
            ScreeningTransition::Applied(CurrentScreeningState {
                title_abstract: Some(ScreeningDecision::Maybe),
                full_text: None,
                full_text_exclusion_reason_id: None,
            })
        );
    }

    #[test]
    fn undo_restores_the_previous_title_abstract_state() {
        let current = CurrentScreeningState {
            title_abstract: Some(ScreeningDecision::Maybe),
            ..CurrentScreeningState::default()
        };
        assert_eq!(
            undo_transition(
                &UndoScreeningTransitionCommand {
                    stage: ScreeningStage::TitleAbstract,
                },
                current,
                CurrentScreeningState::default(),
            ),
            Ok(ScreeningTransition::Applied(
                CurrentScreeningState::default()
            ))
        );
    }

    #[test]
    fn undo_rejects_a_full_text_exclusion_without_a_reason() {
        assert_eq!(
            undo_transition(
                &UndoScreeningTransitionCommand {
                    stage: ScreeningStage::FullText,
                },
                CurrentScreeningState {
                    title_abstract: Some(ScreeningDecision::Include),
                    ..CurrentScreeningState::default()
                },
                CurrentScreeningState {
                    title_abstract: Some(ScreeningDecision::Include),
                    full_text: Some(ScreeningDecision::Exclude),
                    ..CurrentScreeningState::default()
                },
            ),
            Err(ScreeningUndoValidationError::InvalidRestoredState)
        );
    }

    #[test]
    fn full_text_undo_cannot_change_the_title_abstract_decision() {
        assert_eq!(
            undo_transition(
                &UndoScreeningTransitionCommand {
                    stage: ScreeningStage::FullText,
                },
                CurrentScreeningState {
                    title_abstract: Some(ScreeningDecision::Include),
                    ..CurrentScreeningState::default()
                },
                CurrentScreeningState::default(),
            ),
            Err(ScreeningUndoValidationError::FullTextCannotAlterTitleAbstract)
        );
    }

    #[test]
    fn title_undo_clears_dependent_full_text_state() {
        assert_eq!(
            undo_transition(
                &UndoScreeningTransitionCommand {
                    stage: ScreeningStage::TitleAbstract,
                },
                CurrentScreeningState {
                    title_abstract: Some(ScreeningDecision::Maybe),
                    ..CurrentScreeningState::default()
                },
                CurrentScreeningState {
                    title_abstract: Some(ScreeningDecision::Exclude),
                    full_text: Some(ScreeningDecision::Include),
                    ..CurrentScreeningState::default()
                },
            ),
            Ok(ScreeningTransition::Applied(CurrentScreeningState {
                title_abstract: Some(ScreeningDecision::Exclude),
                ..CurrentScreeningState::default()
            }))
        );
    }

    #[test]
    fn actor_constructor_rejects_blank_ids_and_parses_allowed_kinds() {
        assert_eq!(ActorKind::parse("automation"), Some(ActorKind::Automation));
        assert_eq!(
            Actor::new(ActorKind::User, "  "),
            Err(ActorValidationError::BlankId)
        );
        let actor = Actor::new(ActorKind::System, "worker").expect("actor should be valid");
        assert_eq!(actor.kind(), ActorKind::System);
        assert_eq!(actor.id(), "worker");
    }
}
