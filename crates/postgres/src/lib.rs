mod acquisition;
mod ai;
mod appraisal;
mod automations;
mod deduplication;
mod documents;
mod extraction;
mod graph;
mod jobs;
mod legacy_import;
mod prisma;
mod protocol;
mod screening;
mod study;

use sqlx::{
    PgPool,
    migrate::{MigrateError, Migrator},
};

pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

pub async fn migrate(pool: &PgPool) -> Result<(), MigrateError> {
    MIGRATOR.run(pool).await
}

pub use acquisition::{
    AcquisitionError, ImportPersistRequest, ImportPersistResult, ensure_legacy_acquisition_run,
    persist_import,
};
pub use ai::{
    AiDedupeTarget, AiGroundingBlock, AiGroupingReport, AiGroupingStudy, AiProposalCursor,
    AiProposalDecision, AiProposalDecisionRequest, AiProposalError, AiProposalFilters,
    AiProposalRecord, AiProposalResolution, AiScreeningTarget, AiStudyGroupingTarget,
    PostgresAiStore, ReviewedAiProposalPayload, decide_ai_proposal, get_ai_dedupe_target,
    get_ai_proposal, get_ai_screening_target, get_ai_study_grouping_target, insert_model_route,
    list_ai_exclusion_reasons, list_ai_extraction_evidence, list_ai_grounding_blocks,
    list_ai_proposals, persist_document_block_embedding, resolve_ai_proposal,
};
pub use appraisal::{
    AppraisalAssessmentRecord, AppraisalError, AppraisalEvidenceRecord, complete_appraisal,
    complete_appraisal_in_transaction, get_appraisal, list_appraisals,
};
pub use automations::{
    AutomationDispatchResult, AutomationError, AutomationFinalization, begin_next_automation_step,
    complete_automation_step, complete_automation_step_with_output,
    configure_automation_definition, dispatch_automation_trigger, fail_automation_step,
    finalize_automation_run, get_automation_run, list_automation_definitions, list_automation_runs,
    retry_automation_run, start_automation_manually,
};
pub use deduplication::{
    DedupeError, DedupeProposal, DedupeProposalCursor, DedupeRunRequest, DedupeRunSummary,
    ProposalDecisionRequest, ResolutionResult, decide_proposal, list_proposals, resolve_record,
    run_deduplication,
};
pub use documents::{
    CompleteDocumentRetrievalOutcome, DocumentBlockRecord, DocumentPageRecord, DocumentRecord,
    ExclusionReasonRecord, FullTextQueueRecord, MissingFullTextRecord, NewDocument,
    complete_document_retrieval, create_document, enqueue_parse, enqueue_retrieve, get_document,
    get_document_blocks, get_document_by_id, get_document_pages, insert_document_blocks,
    list_documents, list_full_text_queue, list_full_text_reasons, list_missing_full_text,
    mark_document_failed, mark_document_parsing, mark_document_retrieval_failed,
    mark_document_retrieving, persist_parsed_document, search_document_blocks,
};
pub use extraction::{
    ExtractionError, ExtractionValueRecord, apply_data_extraction_in_transaction,
    create_field_definition, list_field_definitions, list_values,
};
pub use graph::{MAX_GRAPH_NODES, load_project_graph, recompute_project_metrics};
pub use jobs::{
    PostgresJobQueue, claim_job, complete_job, enqueue_job, enqueue_job_pool, fail_job, job,
    recover_expired_jobs, renew_job,
};
pub use legacy_import::{LegacyImportCounts, import_legacy};
pub use prisma::{PrismaProjectionError, get_prisma_projection};
pub use protocol::{
    ProtocolActor, ProtocolDocument, ProtocolError, get_protocol_editor, get_published_protocol,
    publish_protocol, save_protocol_draft,
};
pub use screening::{
    ScreeningError, ScreeningHistory, ScreeningHistoryItem, ScreeningProgress, ScreeningQueue,
    ScreeningQueueItem, ScreeningStateSnapshot, get_next_screening_item, get_screening_history,
    get_screening_queue, screen_report, undo_screening,
};
pub use study::{
    StudyDetailRecord, StudyError, StudyEventRecord, StudyListRecord, StudyMembershipRecord,
    StudyRecord, StudyReportRecord, assign_report_to_study, assign_report_to_study_in_transaction,
    classify_study, create_study, create_study_and_assign_report_in_transaction, get_study,
    get_study_for_report, list_studies, list_study_events, remove_report_from_study, rename_study,
};
