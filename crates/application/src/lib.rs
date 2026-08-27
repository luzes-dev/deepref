use deepref_domain::{
    Actor, CurrentScreeningState, ExclusionReasonId, ProjectId, ProtocolVersionId, ReportId,
    ScreenReportTransitionCommand, ScreeningDecision, ScreeningStage, ScreeningTransition,
    ScreeningUndoValidationError, ScreeningValidationError, UndoScreeningTransitionCommand,
    transition, undo_transition,
};

pub mod acquisition;
pub mod appraisal;
pub mod deduplication;
pub mod documents;
pub mod extraction;
pub mod jobs;
pub mod prisma;
pub mod protocol;
pub mod study;

pub use acquisition::{
    CitationProvider, CsvColumnMapping, FullTextResolver, ImportError, ImportParser,
    MetadataProvider, ProviderError, ProviderFuture, RawAuthor, RawIdentifier, RawRecord,
    SearchProvider,
};
pub use appraisal::{
    AnswerSchema, AppraisalAssessmentInput, AppraisalCompleted, AppraisalDefinition,
    AppraisalDefinitionError, AppraisalDomain, AppraisalQuestion, AppraisalValidationError,
    DefinitionId, DefinitionVersion, EvidenceReferenceInput, JudgmentSchema,
    all_appraisal_definitions, get_appraisal_definition, parse_appraisal_definition_resource,
    validate_assessment_input, validate_definition_resource,
    validate_shipped_appraisal_definitions,
};
pub use deduplication::{
    DecideProposalCommand, DedupeCandidate, DedupeDisposition, DedupeProposalCommand, DedupeScore,
    FUZZY_PROPOSAL_THRESHOLD, FUZZY_SHORTLIST_LIMIT, ProposalDecision, ProposalKind,
    RecordResolutionAction, ResolutionCommandError, ResolveRecordCommand, disposition,
    score_candidate,
};
pub use documents::{
    AttachDocumentCommand, DocumentByteStream, DocumentDetailQuery, DocumentFuture,
    DocumentListQuery, DocumentPortError, DocumentQueryError, DocumentRepository,
    DocumentStorageId, DocumentStore, MissingFullTextQuery, OcrEngine, StoredDocumentContent,
};
pub use extraction::{
    ExtractionFieldDefinition, ExtractionFieldType, ExtractionValidationError, ExtractionValue,
};
pub use prisma::{
    NonNegativeCount, PrismaInvariantError, PrismaProjection, PrismaReasonCount, render_prisma_svg,
};
pub use protocol::{
    ProtocolCommandError, ProtocolCriterionCommand, PublishProtocolCommand,
    SaveProtocolDraftCommand, validate_protocol_text,
};
pub use study::{
    AssignReportToStudy, ClassifyStudy, CreateStudy, RemoveReportFromStudy, RenameStudy,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreeningQueueStatus {
    Unscreened,
    Include,
    Exclude,
    Maybe,
    All,
}

impl ScreeningQueueStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unscreened => "unscreened",
            Self::Include => "include",
            Self::Exclude => "exclude",
            Self::Maybe => "maybe",
            Self::All => "all",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "unscreened" => Some(Self::Unscreened),
            "include" => Some(Self::Include),
            "exclude" => Some(Self::Exclude),
            "maybe" => Some(Self::Maybe),
            "all" => Some(Self::All),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreeningQueueSort {
    CreatedAscending,
    CreatedDescending,
    TitleAscending,
    TitleDescending,
    YearAscending,
    YearDescending,
}

impl ScreeningQueueSort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CreatedAscending => "created_asc",
            Self::CreatedDescending => "created_desc",
            Self::TitleAscending => "title_asc",
            Self::TitleDescending => "title_desc",
            Self::YearAscending => "year_asc",
            Self::YearDescending => "year_desc",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "created_asc" => Some(Self::CreatedAscending),
            "created_desc" => Some(Self::CreatedDescending),
            "title_asc" => Some(Self::TitleAscending),
            "title_desc" => Some(Self::TitleDescending),
            "year_asc" => Some(Self::YearAscending),
            "year_desc" => Some(Self::YearDescending),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetScreeningQueueQuery {
    pub project_id: ProjectId,
    pub status: ScreeningQueueStatus,
    pub search: Option<String>,
    pub sort: ScreeningQueueSort,
    pub cursor: Option<String>,
    pub limit: i64,
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
    pub notes: Option<String>,
    pub actor: Actor,
}

impl ScreenReportCommand {
    pub fn validate(
        &self,
        current: CurrentScreeningState,
    ) -> Result<ScreeningTransition, ScreeningValidationError> {
        transition(
            &ScreenReportTransitionCommand {
                stage: self.stage,
                decision: self.decision,
                exclusion_reason_id: self.exclusion_reason_id,
            },
            current,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoScreeningCommand {
    pub project_id: ProjectId,
    pub report_id: ReportId,
    pub stage: ScreeningStage,
    pub protocol_version_id: ProtocolVersionId,
    pub expected_revision: i64,
    pub notes: Option<String>,
    pub actor: Actor,
}

impl UndoScreeningCommand {
    pub fn validate(
        &self,
        current: CurrentScreeningState,
        restored: CurrentScreeningState,
    ) -> Result<ScreeningTransition, ScreeningUndoValidationError> {
        undo_transition(
            &UndoScreeningTransitionCommand { stage: self.stage },
            current,
            restored,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn screen_report_command_keeps_use_case_context_while_delegating_transition_rules() {
        let command = ScreenReportCommand {
            project_id: Uuid::new_v4().into(),
            report_id: Uuid::new_v4().into(),
            stage: ScreeningStage::TitleAbstract,
            decision: ScreeningDecision::Maybe,
            exclusion_reason_id: None,
            protocol_version_id: Uuid::new_v4().into(),
            expected_revision: 3,
            notes: None,
            actor: Actor::new(deepref_domain::ActorKind::User, "test")
                .expect("test actor should be valid"),
        };

        assert_eq!(
            command.validate(CurrentScreeningState::default()),
            Ok(ScreeningTransition::Applied(CurrentScreeningState {
                title_abstract: Some(ScreeningDecision::Maybe),
                ..CurrentScreeningState::default()
            }))
        );
        assert_eq!(command.expected_revision, 3);
    }
}
