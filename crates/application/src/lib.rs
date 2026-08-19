use deepref_domain::{
    CurrentScreeningState, ExclusionReasonId, ProjectId, ProtocolVersionId, ReportId,
    ScreenReportTransitionCommand, ScreeningDecision, ScreeningStage, ScreeningTransition,
    ScreeningValidationError, transition,
};

pub mod acquisition;
pub mod deduplication;
pub mod jobs;

pub use acquisition::{
    CitationProvider, CsvColumnMapping, FullTextResolver, ImportError, ImportParser,
    MetadataProvider, ProviderError, ProviderFuture, RawAuthor, RawIdentifier, RawRecord,
    SearchProvider,
};
pub use deduplication::{
    DecideProposalCommand, DedupeCandidate, DedupeDisposition, DedupeProposalCommand, DedupeScore,
    FUZZY_PROPOSAL_THRESHOLD, FUZZY_SHORTLIST_LIMIT, ProposalDecision, ProposalKind,
    RecordResolutionAction, ResolutionCommandError, ResolveRecordCommand, disposition,
    score_candidate,
};

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
