mod acquisition;
mod ai;
mod appraisal;
mod deduplication;
mod documents;
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
    PostgresAiStore, insert_model_route, persist_document_block_embedding, resolve_ai_proposal,
};
pub use appraisal::{
    AppraisalAssessmentRecord, AppraisalError, AppraisalEvidenceRecord, complete_appraisal,
    get_appraisal, list_appraisals,
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
    StudyRecord, StudyReportRecord, assign_report_to_study, classify_study, create_study,
    get_study, get_study_for_report, list_studies, list_study_events, remove_report_from_study,
    rename_study,
};
