//! The seam between the workflow engine and the project's AI autonomy
//! settings.
//!
//! Every block that would change review state (record a screening decision,
//! group records into a study, accept an AI result, ...) asks an
//! [`AutonomyGate`] how far it may go before it acts:
//!
//! * [`AutonomyLevel::Off`]            - do nothing and say so in the run log.
//! * [`AutonomyLevel::Suggest`]        - create a proposal / notification a
//!   person must accept; never change review state.
//! * [`AutonomyLevel::SecondReviewer`] - record the result as the automation's
//!   own independent opinion; a person still makes the final call.
//! * [`AutonomyLevel::Act`]            - execute the change, record it in the
//!   activity feed so it can be undone.
//!
//! The workflow engine ships [`DefaultAutonomyGate`], which always answers
//! `Suggest`. The autonomy feature replaces it with an implementation that
//! reads the per-project, per-task setting; the worker receives the gate as an
//! `Arc<dyn AutonomyGate>`, so no engine code changes when it does.

use std::{future::Future, pin::Pin};

use deepref_domain::ProjectId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyLevel {
    Off,
    Suggest,
    SecondReviewer,
    Act,
}

impl AutonomyLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Suggest => "suggest",
            Self::SecondReviewer => "second_reviewer",
            Self::Act => "act",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "suggest" => Some(Self::Suggest),
            "second_reviewer" => Some(Self::SecondReviewer),
            "act" => Some(Self::Act),
            _ => None,
        }
    }
}

/// The kinds of work whose autonomy is configured per project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyTask {
    ExactDuplicates,
    FuzzyDuplicates,
    TitleAbstractScreening,
    FullTextScreening,
    Extraction,
    Appraisal,
    StudyGrouping,
}

impl AutonomyTask {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExactDuplicates => "exact_duplicates",
            Self::FuzzyDuplicates => "fuzzy_duplicates",
            Self::TitleAbstractScreening => "title_abstract_screening",
            Self::FullTextScreening => "full_text_screening",
            Self::Extraction => "extraction",
            Self::Appraisal => "appraisal",
            Self::StudyGrouping => "study_grouping",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|task| task.as_str() == value)
    }

    /// Every task whose autonomy can be configured, in display order.
    pub const ALL: [Self; 7] = [
        Self::ExactDuplicates,
        Self::FuzzyDuplicates,
        Self::TitleAbstractScreening,
        Self::FullTextScreening,
        Self::Extraction,
        Self::Appraisal,
        Self::StudyGrouping,
    ];

    /// The level a project starts with.
    pub const fn default_level(self) -> AutonomyLevel {
        match self {
            Self::ExactDuplicates | Self::Extraction => AutonomyLevel::Act,
            Self::FuzzyDuplicates | Self::Appraisal | Self::StudyGrouping => AutonomyLevel::Suggest,
            Self::TitleAbstractScreening | Self::FullTextScreening => AutonomyLevel::SecondReviewer,
        }
    }

    /// Levels that make sense for this task. The AI never finalizes a
    /// screening decision alone, and appraisal / grouping stay suggestions.
    pub const fn allows(self, level: AutonomyLevel) -> bool {
        match self {
            Self::TitleAbstractScreening | Self::FullTextScreening => {
                !matches!(level, AutonomyLevel::Act)
            }
            Self::Appraisal | Self::StudyGrouping => {
                matches!(level, AutonomyLevel::Off | AutonomyLevel::Suggest)
            }
            // Exact duplicates share durable identifiers, so they cannot be
            // kept apart: the choice is between merging and suggesting.
            Self::ExactDuplicates => {
                matches!(level, AutonomyLevel::Suggest | AutonomyLevel::Act)
            }
            Self::FuzzyDuplicates | Self::Extraction => {
                !matches!(level, AutonomyLevel::SecondReviewer)
            }
        }
    }
}

/// The opinion an automation records as its second-reviewer opinion for a
/// screening suggestion of this kind. `insufficient_evidence` is not an
/// opinion, so it has none and stays a suggestion.
pub fn second_reviewer_opinion(kind: &str) -> Option<&'static str> {
    match kind {
        "include" => Some("include"),
        "exclude" => Some("exclude"),
        "maybe" => Some("maybe"),
        _ => None,
    }
}

/// Work that is never automatic and therefore has no configurable level.
pub const LOCKED_TASKS: [&str; 2] = ["protocol_publishing", "final_exclusion"];

#[derive(Debug, Error)]
pub enum AutonomyError {
    #[error("the autonomy setting could not be read: {0}")]
    Unavailable(String),
}

pub type AutonomyFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Decides how autonomously a workflow block may act for one project.
///
/// Implementations must be cheap to call (one call per block execution) and
/// must fail closed: when the setting cannot be read return an error and the
/// block fails with a retryable message rather than guessing.
pub trait AutonomyGate: Send + Sync {
    fn decide<'a>(
        &'a self,
        task: AutonomyTask,
        project: ProjectId,
    ) -> AutonomyFuture<'a, Result<AutonomyLevel, AutonomyError>>;
}

/// Always `Suggest`: workflows create proposals and never act on their own.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultAutonomyGate;

impl AutonomyGate for DefaultAutonomyGate {
    fn decide<'a>(
        &'a self,
        _task: AutonomyTask,
        _project: ProjectId,
    ) -> AutonomyFuture<'a, Result<AutonomyLevel, AutonomyError>> {
        Box::pin(async { Ok(AutonomyLevel::Suggest) })
    }
}

#[cfg(test)]
mod tests {
    use std::task::{Context, Poll, Waker};

    use uuid::Uuid;

    use super::*;

    #[test]
    fn defaults_are_allowed_and_screening_never_acts() {
        for task in AutonomyTask::ALL {
            assert!(task.allows(task.default_level()), "{task:?}");
            assert_eq!(AutonomyTask::parse(task.as_str()), Some(task));
        }
        assert!(!AutonomyTask::TitleAbstractScreening.allows(AutonomyLevel::Act));
        assert!(!AutonomyTask::FullTextScreening.allows(AutonomyLevel::Act));
        assert_eq!(
            AutonomyTask::ExactDuplicates.default_level(),
            AutonomyLevel::Act
        );
        assert_eq!(
            AutonomyTask::FuzzyDuplicates.default_level(),
            AutonomyLevel::Suggest
        );
    }

    #[test]
    fn default_gate_suggests() {
        let gate = DefaultAutonomyGate;
        let mut future = gate.decide(
            AutonomyTask::TitleAbstractScreening,
            ProjectId::new(Uuid::new_v4()),
        );
        let mut context = Context::from_waker(Waker::noop());
        match future.as_mut().poll(&mut context) {
            Poll::Ready(Ok(level)) => assert_eq!(level, AutonomyLevel::Suggest),
            _ => unreachable!("default gate resolves immediately"),
        }
    }
}
