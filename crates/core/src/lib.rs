mod article;
mod doi;
mod evidence;
mod ingestion;
mod project;
mod reference;
mod screening;
mod work;

pub use article::ArticleSummary;
pub use doi::{DoiError, normalize_doi};
pub use evidence::{
    AcquisitionRunId, AiProposalId, AiRunId, AutomationRunId, DocumentBlockId, DocumentId,
    EligibilityCriterionId, ExclusionReasonId, IdentifierScheme, ProtocolVersionId, Record, RecordId,
    Report, ReportId, ReportIdentifier, ScreeningEventId, Study, StudyId,
};
pub use ingestion::{Ingestion, IngestionItemStatus, IngestionStatus};
pub use project::Project;
pub use reference::Reference;
pub use screening::{
    Actor, ScreeningCommand, ScreeningDecision, ScreeningError, ScreeningEvent, ScreeningStage,
    ScreeningState, ScreeningStatus,
};
pub use work::{FetchStatus, Work, WorkWithReferences};
