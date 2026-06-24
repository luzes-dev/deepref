mod article;
mod doi;
mod ingestion;
mod project;
mod reference;
mod work;

pub use article::ArticleSummary;
pub use doi::{DoiError, normalize_doi};
pub use ingestion::{Ingestion, IngestionItemStatus, IngestionStatus};
pub use project::Project;
pub use reference::Reference;
pub use work::{FetchStatus, Work, WorkWithReferences};
