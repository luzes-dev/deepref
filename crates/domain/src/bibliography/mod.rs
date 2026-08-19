mod identifier;
mod model;
mod title;

pub use identifier::{
    DoiError, IdentifierError, IdentifierScheme, ReportIdentifier, normalize_doi,
};
pub use model::{Citation, Record, RecordId, Report, ReportId, Study, StudyId, Title, TitleError};
pub use title::normalize_bibliography_title;
