use std::{future::Future, pin::Pin};

use bytes::Bytes;
use deepref_domain::{
    Actor, DocumentBlock, DocumentId, DocumentMetadata, DocumentSource, ProjectId, ReportId,
};
use futures::Stream;
use thiserror::Error;
use uuid::Uuid;

pub type DocumentFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, DocumentPortError>> + Send + 'a>>;
pub type DocumentByteStream<'a> =
    Pin<Box<dyn Stream<Item = Result<Bytes, DocumentPortError>> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DocumentStorageId(Uuid);

impl DocumentStorageId {
    pub const fn new(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredDocumentContent {
    pub storage_id: DocumentStorageId,
    pub byte_size: u64,
    pub sha256: String,
}

pub trait DocumentStore: Send + Sync {
    fn write_pdf<'a>(
        &'a self,
        content: DocumentByteStream<'a>,
    ) -> DocumentFuture<'a, StoredDocumentContent>;
    fn read_pdf<'a>(&'a self, id: DocumentStorageId) -> DocumentFuture<'a, DocumentByteStream<'a>>;
    fn delete_pdf<'a>(&'a self, id: DocumentStorageId) -> DocumentFuture<'a, ()>;
}

pub trait DocumentRepository: Send + Sync {
    fn list<'a>(&'a self, query: DocumentListQuery) -> DocumentFuture<'a, Vec<DocumentMetadata>>;
    fn detail<'a>(&'a self, query: DocumentDetailQuery) -> DocumentFuture<'a, DocumentMetadata>;
    fn blocks<'a>(&'a self, query: DocumentDetailQuery) -> DocumentFuture<'a, Vec<DocumentBlock>>;
}

pub trait OcrEngine: Send + Sync {
    fn enqueue<'a>(&'a self, document_id: DocumentId) -> DocumentFuture<'a, ()>;
}

#[derive(Debug, Error)]
pub enum DocumentPortError {
    #[error("document was not found")]
    NotFound,
    #[error("document content exceeds the configured limit")]
    TooLarge,
    #[error("document operation failed: {0}")]
    Operation(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachDocumentCommand {
    pub project_id: ProjectId,
    pub report_id: ReportId,
    pub source: DocumentSource,
    pub original_filename: Option<String>,
    pub mime_type: Option<String>,
    pub external_url: Option<String>,
    pub actor: Actor,
}

impl AttachDocumentCommand {
    pub fn validate(&self) -> Result<(), DocumentQueryError> {
        if self
            .original_filename
            .as_deref()
            .is_some_and(|name| name.len() > 255)
        {
            return Err(DocumentQueryError::InvalidFilename);
        }
        if matches!(self.source, DocumentSource::ExternalUrl)
            && self.external_url.as_deref().is_none_or(str::is_empty)
        {
            return Err(DocumentQueryError::ExternalUrlRequired);
        }
        if matches!(self.source, DocumentSource::ExternalUrl)
            && !self
                .external_url
                .as_deref()
                .is_some_and(|url| url.starts_with("https://"))
        {
            return Err(DocumentQueryError::ExternalUrlMustUseHttps);
        }
        if matches!(self.source, DocumentSource::Upload)
            && self
                .mime_type
                .as_deref()
                .is_some_and(|mime| !mime.eq_ignore_ascii_case("application/pdf"))
        {
            return Err(DocumentQueryError::InvalidMimeType);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentListQuery {
    pub project_id: ProjectId,
    pub report_id: Option<ReportId>,
    pub status: Option<String>,
    pub limit: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentDetailQuery {
    pub project_id: ProjectId,
    pub report_id: ReportId,
    pub document_id: DocumentId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissingFullTextQuery {
    pub project_id: ProjectId,
    pub limit: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DocumentQueryError {
    #[error("document filename is too long")]
    InvalidFilename,
    #[error("an external URL is required for an external document")]
    ExternalUrlRequired,
    #[error("document size exceeds the configured limit")]
    TooLarge,
    #[error("document content is not a PDF")]
    InvalidPdf,
    #[error("external document URLs must use HTTPS")]
    ExternalUrlMustUseHttps,
    #[error("uploaded documents must use application/pdf")]
    InvalidMimeType,
}
