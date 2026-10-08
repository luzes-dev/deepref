mod acquisition;
mod activity;
mod agent_tools;
mod ai;
mod ai_autonomy_apply;
mod ai_reviewer;
mod ai_usage;
mod appraisal;
mod assistant;
mod assistant_plans;
mod audit_export;
mod automations;
mod autonomy;
mod deduplication;
mod documents;
mod extraction;
mod graph;
mod jobs;
mod legacy_import;
mod notifications;
mod pmid_import;
mod prisma;
mod project_deletion;
mod protocol;
mod review_calibration;
mod review_completion;
mod review_preparation;
mod review_run_setup;
mod review_runs;
mod screening;
mod second_review;
mod study;
mod worker_runtime;
mod workflows;

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
pub use activity::{
    ActivityError, ActivityFilters, ActivityOverview, ActivityRecord, BatchUndoResult, NewActivity,
    activity_overview, get_activity, list_activity, record_activity,
    record_activity_in_transaction, undo_activity, undo_batch,
};
pub use agent_tools::{
    AgentAppraisalEvidence, AgentAppraisalRecord, AgentDocumentBlockRecord, AgentDocumentSummary,
    AgentReadError, AgentReportIdentifier, AgentReportRecord, AgentSectionSummary,
    decode_html_entities, get_agent_project_overview, get_agent_report, get_agent_screening_state,
    get_latest_agent_appraisal, list_agent_reports_by_screening, list_agent_studies,
    project_exists, read_agent_document_blocks, search_agent_document, search_agent_reports,
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
pub use ai_autonomy_apply::{AutonomyOutcome, apply_autonomy_for_proposal};
pub use ai_reviewer::{
    NewReviewerDecision, ResolveConflict, ReviewerDecisionRecord, ReviewerError, StageAgreement,
    cohens_kappa, insert_reviewer_decision_in_transaction, list_reviewer_decisions,
    record_workflow_reviewer_decision, resolve_reviewer_conflict, reviewer_agreement,
};
pub use ai_usage::{
    AiUsageError, DEFAULT_AI_MONTHLY_BUDGET_MICROS, DefaultRoutes, PostgresUsageLedger,
    ensure_default_model_routes, get_ai_budget, set_ai_budget,
};
pub use appraisal::{
    AppraisalAssessmentRecord, AppraisalError, AppraisalEvidenceRecord, complete_appraisal,
    complete_appraisal_in_transaction, get_appraisal, list_appraisals,
};
pub use assistant::{
    AppendAssistantMessage, AssistantConversationRecord, AssistantError, AssistantMessageRecord,
    append_assistant_message, create_assistant_conversation, delete_assistant_conversation,
    get_assistant_conversation, list_assistant_conversations, list_assistant_messages,
};
pub use assistant_plans::{
    AssistantPlanRecord, NewAssistantPlan, claim_assistant_plan, create_assistant_plan,
    finish_assistant_plan, get_assistant_plan, update_assistant_plan_results,
};
pub use audit_export::{AuditExportRow, load_audit_export_rows};
pub use automations::{
    AutomationDispatchResult, AutomationError, AutomationFinalization, begin_next_automation_step,
    complete_automation_step, complete_automation_step_with_output,
    configure_automation_definition, dispatch_automation_domain_event, dispatch_automation_trigger,
    fail_automation_step, finalize_automation_run, get_automation_run, list_automation_definitions,
    list_automation_runs, retry_automation_run, start_automation_manually,
};
pub use autonomy::{
    AutonomySetting, AutonomyStoreError, ProjectAutonomyGate, get_autonomy_settings,
    resolve_autonomy_level, set_autonomy_level,
};
pub use deduplication::{
    DedupeError, DedupeProposal, DedupeProposalCursor, DedupeRunRequest, DedupeRunSummary,
    IMPORT_DEDUPLICATION_JOB_KIND, ProposalDecisionRequest, ResolutionResult, decide_proposal,
    enqueue_import_deduplication, list_proposals, resolve_record, run_deduplication,
};
pub use documents::{
    CompleteDocumentRetrievalOutcome, DocumentBlockRecord, DocumentPageRecord, DocumentRecord,
    DocumentReferenceRecord, DocumentSectionRecord, DocumentStructure, ExclusionReasonRecord,
    FullTextQueueRecord, MissingFullTextRecord, NewDocument, NewDocumentReference,
    NewDocumentSection, acknowledge_identity_mismatch, complete_document_retrieval,
    create_document, delete_document, enqueue_parse, enqueue_retrieve,
    find_failed_document_by_hash, get_document, get_document_blocks, get_document_by_id,
    get_document_pages, insert_document_blocks, list_document_references, list_document_sections,
    list_documents, list_full_text_queue, list_full_text_reasons, list_missing_full_text,
    mark_document_failed, mark_document_parsing, mark_document_retrieval_failed,
    mark_document_retrieving, persist_parsed_document, persist_parsed_document_with_structure,
    record_identity_check, report_identity, requeue_document_processing, search_document_blocks,
};
pub use extraction::{
    ExtractionApplyOptions, ExtractionApplyOutcome, ExtractionError, ExtractionFieldUpdate,
    ExtractionValueRecord, ManualExtractionEvidence, ManualExtractionValue,
    apply_data_extraction_in_transaction, clear_value, confirm_value, create_field_definition,
    list_field_definitions, list_values, record_manual_value, update_field_definition,
};
pub use graph::{
    MAX_GRAPH_NODES, enqueue_metrics_recompute, load_project_graph, recompute_project_metrics,
};
pub use jobs::{
    PostgresJobQueue, claim_job, complete_job, enqueue_job, enqueue_job_pool, fail_job,
    get_claimed_automation_job_project_id_for_run, job, recover_expired_jobs, renew_job,
};
pub use legacy_import::{LegacyImportCounts, import_legacy};
pub use notifications::{
    MarkNotificationsRead, NotificationDraft, NotificationPage, NotificationRecord,
    NotificationSeverity, NotificationUnreadSummary, list_notifications, mark_notifications_read,
    record_notification, record_notification_in_transaction,
    record_run_notification_once_in_transaction, unread_summary,
};
pub use pmid_import::{
    PMID_IMPORT_JOB_KIND, PMID_IMPORT_SOURCE, PMID_IMPORT_STRATEGY, PmidImportRequest,
    PmidImportResult, PmidItemStatus, PmidOutcome, PmidRun, PmidRunCounts, complete_pmid_run,
    create_pmid_import, load_pmid_run, pmid_run_counts, pmids_in_project, queued_pmids,
    save_pmid_outcomes, start_pmid_run,
};
pub use prisma::{PrismaProjectionError, get_prisma_projection};
pub use project_deletion::delete_project_in_transaction;
pub use protocol::{
    ProtocolActor, ProtocolDocument, ProtocolError, get_protocol_editor, get_published_protocol,
    list_protocol_versions, publish_protocol, save_protocol_draft,
};
pub use review_calibration::{
    ReviewCalibrationBundleInput, ReviewCalibrationError, ReviewCalibrationStatus,
    insert_review_calibration_bundle,
};
pub use review_completion::{
    ReviewOutcome, ReviewOutcomeCompletion, bind_review_step_acceptance, complete_review_attempt,
    complete_review_outcome, complete_review_step,
};
pub use review_preparation::{
    PostgresReviewScheduler, ReviewPreparationError, schedule_appraisal_prefill_review,
    schedule_data_extraction_review, schedule_duplicate_detection_review,
    schedule_screening_review, schedule_study_classification_review,
    schedule_study_grouping_review,
};
pub use review_runs::{
    AcceptedReviewAttempt, LeasedReviewRun, PostgresReviewError, PreparedReviewRun,
    ReviewAttemptCompletion, ReviewAttemptStart, ReviewFinalization, begin_review_attempt,
    block_review_run, fail_review_attempt, fail_review_run, finalize_review_proposal,
    get_review_run, get_review_runs, load_leased_review_run, mark_review_run_running,
    schedule_prepared_review_run,
};
pub use screening::{
    ScreeningError, ScreeningHistory, ScreeningHistoryItem, ScreeningProgress, ScreeningQueue,
    ScreeningQueueItem, ScreeningStateSnapshot, get_next_screening_item, get_screening_history,
    get_screening_queue, screen_report, undo_screening,
};
pub use second_review::{
    SECOND_REVIEW_BATCH, SecondReviewStatus, SecondReviewSweep, second_review_status,
    sweep_second_reviews,
};
pub use study::{
    StudyDetailRecord, StudyError, StudyEventRecord, StudyListRecord, StudyMembershipRecord,
    StudyRecord, StudyReportRecord, UngroupedReportRecord, assign_report_to_study,
    assign_report_to_study_in_transaction, classify_study, create_study,
    create_study_and_assign_report_in_transaction, get_study, get_study_for_report,
    list_reports_for_studies, list_studies, list_study_events, list_ungrouped_included_reports,
    remove_report_from_study, rename_study,
};
pub use worker_runtime::{
    MissingIngestionWork, WorkerLeaseRecovery, find_missing_ingestion_work,
    recover_expired_worker_state, reserve_provider_permit,
};
pub use workflows::*;
