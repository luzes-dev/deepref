mod acquisition;
mod deduplication;
mod graph;
mod jobs;
mod legacy_import;
mod protocol;
mod screening;

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
pub use deduplication::{
    DedupeError, DedupeProposal, DedupeProposalCursor, DedupeRunRequest, DedupeRunSummary,
    ProposalDecisionRequest, ResolutionResult, decide_proposal, list_proposals, resolve_record,
    run_deduplication,
};
pub use graph::{MAX_GRAPH_NODES, load_project_graph, recompute_project_metrics};
pub use jobs::{
    PostgresJobQueue, claim_job, complete_job, enqueue_job, enqueue_job_pool, fail_job, job,
    recompute_prisma_snapshot, recover_expired_jobs, renew_job,
};
pub use legacy_import::{LegacyImportCounts, import_legacy};
pub use protocol::{
    ProtocolActor, ProtocolDocument, ProtocolError, get_protocol_editor, get_published_protocol,
    publish_protocol, save_protocol_draft,
};
pub use screening::{
    ScreeningError, ScreeningHistory, ScreeningHistoryItem, ScreeningProgress, ScreeningQueue,
    ScreeningQueueItem, ScreeningStateSnapshot, get_next_screening_item, get_screening_history,
    get_screening_queue, screen_report, undo_screening,
};
