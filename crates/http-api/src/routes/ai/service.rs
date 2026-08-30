use super::{
    GenerateAppraisalPrefillRequest, GenerateScreeningRequest, create_appraisal_prefill_proposal,
    create_duplicate_proposal, create_screening_proposal, create_study_grouping_proposal,
};
use crate::{error::ApiError, state::AppState};
use deepref_postgres::AiProposalRecord;
use uuid::Uuid;

pub(crate) struct ScreeningReviewCommand {
    pub project_id: Uuid,
    pub report_id: Uuid,
    pub input: GenerateScreeningRequest,
}

pub(crate) struct StudyGroupingReviewCommand {
    pub project_id: Uuid,
    pub report_id: Uuid,
}

pub(crate) struct AppraisalPrefillReviewCommand {
    pub project_id: Uuid,
    pub report_id: Uuid,
    pub input: GenerateAppraisalPrefillRequest,
}

pub(crate) struct DuplicateReviewCommand {
    pub project_id: Uuid,
    pub record_id: Uuid,
    pub candidate_report_id: Uuid,
}

/// Internal application boundary for evidence-grounded AI review proposals.
///
/// HTTP and assistant adapters construct typed commands. Proposal generation remains
/// centralized here so future callers cannot accidentally swap project/report identities
/// or bypass the same review orchestration path.
pub(crate) struct AiReviewService<'state> {
    state: &'state AppState,
}

impl<'state> AiReviewService<'state> {
    pub(crate) fn new(state: &'state AppState) -> Self {
        Self { state }
    }

    pub(crate) async fn screening(
        &self,
        command: ScreeningReviewCommand,
    ) -> Result<AiProposalRecord, ApiError> {
        create_screening_proposal(
            self.state,
            command.project_id,
            command.report_id,
            command.input,
        )
        .await
    }

    pub(crate) async fn study_grouping(
        &self,
        command: StudyGroupingReviewCommand,
    ) -> Result<AiProposalRecord, ApiError> {
        create_study_grouping_proposal(self.state, command.project_id, command.report_id).await
    }

    pub(crate) async fn appraisal_prefill(
        &self,
        command: AppraisalPrefillReviewCommand,
    ) -> Result<AiProposalRecord, ApiError> {
        create_appraisal_prefill_proposal(
            self.state,
            command.project_id,
            command.report_id,
            command.input,
        )
        .await
    }

    pub(crate) async fn duplicate(
        &self,
        command: DuplicateReviewCommand,
    ) -> Result<AiProposalRecord, ApiError> {
        create_duplicate_proposal(
            self.state,
            command.project_id,
            command.record_id,
            command.candidate_report_id,
        )
        .await
    }
}
