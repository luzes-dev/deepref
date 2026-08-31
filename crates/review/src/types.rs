use std::{fmt, future::Future, pin::Pin};

use chrono::{DateTime, Utc};
use deepref_domain::{
    Actor, ProjectId, ProtocolVersionId, RecordId, ReportId, ScreeningStage, StudyId,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub type ReviewFuture<'a, T, E = ReviewError> =
    Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDefinitionKey {
    Screening,
    DuplicateDetection,
    StudyClassification,
    StudyGrouping,
    AppraisalPrefill,
    DataExtraction,
}

impl ReviewDefinitionKey {
    pub const ALL: [Self; 6] = [
        Self::Screening,
        Self::DuplicateDetection,
        Self::StudyClassification,
        Self::StudyGrouping,
        Self::AppraisalPrefill,
        Self::DataExtraction,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Screening => "screening",
            Self::DuplicateDetection => "duplicate_detection",
            Self::StudyClassification => "study_classification",
            Self::StudyGrouping => "study_grouping",
            Self::AppraisalPrefill => "appraisal_prefill",
            Self::DataExtraction => "data_extraction",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|definition| definition.as_str() == value)
    }
}

impl fmt::Display for ReviewDefinitionKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReviewRunId(Uuid);

impl ReviewRunId {
    pub fn new(value: Uuid) -> Result<Self, ReviewError> {
        if value.is_nil() {
            return Err(ReviewError::InvalidRunId);
        }
        Ok(Self(value))
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl From<ReviewRunId> for Uuid {
    fn from(value: ReviewRunId) -> Self {
        value.as_uuid()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CalibrationBundleId(Uuid);

impl CalibrationBundleId {
    pub fn new(value: Uuid) -> Result<Self, ReviewError> {
        if value.is_nil() {
            return Err(ReviewError::InvalidCalibrationBundleId);
        }
        Ok(Self(value))
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewSubject {
    Screening {
        report_id: ReportId,
        stage: ScreeningStage,
        protocol_version_id: ProtocolVersionId,
        expected_revision: i64,
    },
    DuplicateDetection {
        record_id: RecordId,
        candidate_report_id: ReportId,
    },
    StudyClassification {
        study_id: StudyId,
        expected_revision: i64,
    },
    StudyGrouping {
        report_id: ReportId,
        expected_previous_study_id: Option<StudyId>,
        expected_previous_study_revision: Option<i64>,
    },
    AppraisalPrefill {
        report_id: ReportId,
        definition_id: String,
        definition_version: u32,
    },
    DataExtraction {
        study_id: StudyId,
        field_set_version: u32,
    },
}

impl ReviewSubject {
    pub const fn definition_key(&self) -> ReviewDefinitionKey {
        match self {
            Self::Screening { .. } => ReviewDefinitionKey::Screening,
            Self::DuplicateDetection { .. } => ReviewDefinitionKey::DuplicateDetection,
            Self::StudyClassification { .. } => ReviewDefinitionKey::StudyClassification,
            Self::StudyGrouping { .. } => ReviewDefinitionKey::StudyGrouping,
            Self::AppraisalPrefill { .. } => ReviewDefinitionKey::AppraisalPrefill,
            Self::DataExtraction { .. } => ReviewDefinitionKey::DataExtraction,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewOrigin {
    ReviewerRequested,
    AutomationTriggered {
        calibration_bundle_id: CalibrationBundleId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleReviewRun {
    pub project_id: ProjectId,
    pub definition: ReviewDefinitionKey,
    pub subject: ReviewSubject,
    pub origin: ReviewOrigin,
    pub actor: Actor,
}

impl ScheduleReviewRun {
    pub fn validate(&self) -> Result<(), ReviewError> {
        if self.project_id.as_uuid().is_nil() {
            return Err(ReviewError::InvalidProjectId);
        }
        if self.definition != self.subject.definition_key() {
            return Err(ReviewError::SubjectDefinitionMismatch {
                definition: self.definition,
                subject: self.subject.definition_key(),
            });
        }
        match &self.subject {
            ReviewSubject::Screening {
                expected_revision, ..
            }
            | ReviewSubject::StudyClassification {
                expected_revision, ..
            } if *expected_revision < 0 => Err(ReviewError::InvalidExpectedRevision),
            ReviewSubject::StudyGrouping {
                expected_previous_study_id,
                expected_previous_study_revision,
                ..
            } if expected_previous_study_id.is_some()
                != expected_previous_study_revision.is_some() =>
            {
                Err(ReviewError::IncompleteStudyRevision)
            }
            ReviewSubject::StudyGrouping {
                expected_previous_study_revision: Some(revision),
                ..
            } if *revision < 0 => Err(ReviewError::InvalidExpectedRevision),
            ReviewSubject::AppraisalPrefill {
                definition_id,
                definition_version,
                ..
            } if definition_id.trim().is_empty() || *definition_version == 0 => {
                Err(ReviewError::InvalidAppraisalDefinition)
            }
            ReviewSubject::DataExtraction {
                field_set_version, ..
            } if *field_set_version == 0 => Err(ReviewError::InvalidFieldSetVersion),
            ReviewSubject::Screening { .. }
            | ReviewSubject::DuplicateDetection { .. }
            | ReviewSubject::StudyClassification { .. }
            | ReviewSubject::StudyGrouping { .. }
            | ReviewSubject::AppraisalPrefill { .. }
            | ReviewSubject::DataExtraction { .. } => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewBlockCode {
    SubjectChanged,
    SourceIncomplete,
    CalibrationMissing,
    CalibrationFailed,
    CalibrationStale,
    HumanAdjudicationRequired,
    RepairBudgetExhausted,
}

impl ReviewBlockCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SubjectChanged => "subject_changed",
            Self::SourceIncomplete => "source_incomplete",
            Self::CalibrationMissing => "calibration_missing",
            Self::CalibrationFailed => "calibration_failed",
            Self::CalibrationStale => "calibration_stale",
            Self::HumanAdjudicationRequired => "human_adjudication_required",
            Self::RepairBudgetExhausted => "repair_budget_exhausted",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "subject_changed" => Some(Self::SubjectChanged),
            "source_incomplete" => Some(Self::SourceIncomplete),
            "calibration_missing" => Some(Self::CalibrationMissing),
            "calibration_failed" => Some(Self::CalibrationFailed),
            "calibration_stale" => Some(Self::CalibrationStale),
            "human_adjudication_required" => Some(Self::HumanAdjudicationRequired),
            "repair_budget_exhausted" => Some(Self::RepairBudgetExhausted),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewRunState {
    Queued,
    Running,
    Blocked {
        code: ReviewBlockCode,
        message: String,
    },
    Failed {
        code: String,
        message: String,
    },
    Completed {
        proposal_id: Uuid,
    },
}

impl ReviewRunState {
    pub const fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Blocked { .. } | Self::Failed { .. } | Self::Completed { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRunSnapshot {
    pub id: ReviewRunId,
    pub project_id: ProjectId,
    pub definition: ReviewDefinitionKey,
    pub subject: ReviewSubject,
    pub origin: ReviewOrigin,
    pub state: ReviewRunState,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

pub trait ReviewScheduler: Send + Sync {
    type Error;

    fn schedule<'a>(
        &'a self,
        command: ScheduleReviewRun,
    ) -> ReviewFuture<'a, ReviewRunSnapshot, Self::Error>;

    fn get<'a>(
        &'a self,
        project_id: ProjectId,
        run_id: ReviewRunId,
    ) -> ReviewFuture<'a, ReviewRunSnapshot, Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ReviewError {
    #[error("review run id must not be nil")]
    InvalidRunId,
    #[error("calibration bundle id must not be nil")]
    InvalidCalibrationBundleId,
    #[error("project id must not be nil")]
    InvalidProjectId,
    #[error("review subject {subject} does not match definition {definition}")]
    SubjectDefinitionMismatch {
        definition: ReviewDefinitionKey,
        subject: ReviewDefinitionKey,
    },
    #[error("expected revision must not be negative")]
    InvalidExpectedRevision,
    #[error("existing study identity and revision must be supplied together")]
    IncompleteStudyRevision,
    #[error("appraisal definition identity is invalid")]
    InvalidAppraisalDefinition,
    #[error("field set version must be positive")]
    InvalidFieldSetVersion,
    #[error("review definition is invalid: {0}")]
    InvalidDefinition(String),
    #[error("review workflow is invalid: {0}")]
    InvalidWorkflow(String),
    #[error("review hash input is invalid: {0}")]
    InvalidHash(String),
    #[error("review persistence failed: {0}")]
    Persistence(String),
    #[error("review execution failed: {0}")]
    Execution(String),
}
