pub use deepref_config::DEFAULT_MAX_DOCUMENT_BYTES;
use deepref_config::{DocumentConfig, DocumentStorageBackend, ExposeSecret};
use std::sync::Arc;

use bytes::{Buf, Bytes, BytesMut};
use futures::{Stream, StreamExt};
use object_store::{
    GetResult, ObjectStore as ObjectStoreTrait, ObjectStoreExt, PutPayload, path::Path,
};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

mod chars;
mod identity;
mod layout;
mod parser;
mod pipeline;
mod remote;
mod structure;

pub use identity::{IdentityCheck, IdentityVerdict, PdfIdentity, ReportIdentity, check_identity};
pub use parser::{DocumentParser, ParserLimits, PdfParserError, PdfiumParser, parse_pdf_file};
pub use remote::{
    FetchPolicy, HttpsPdfFetcher, RemoteDocumentFetcher, RemoteFetchError, RemoteFetchFuture,
    validate_external_url,
};
pub use structure::is_caption as is_caption_text;

pub const PARSER_VERSION: &str = "deepref-pdfium-0.9-v4";

#[derive(Debug, Error)]
pub enum StoreConfigError {
    #[error("document storage root could not be initialized: {0}")]
    ObjectStore(#[from] object_store::Error),
    #[error("document storage configuration is invalid: {0}")]
    Invalid(&'static str),
    #[error("document storage root could not be initialized: {0}")]
    Initialization(String),
}

pub struct StoreConfig {
    inner: Arc<dyn ObjectStoreTrait>,
}

impl StoreConfig {
    pub fn local(root: impl AsRef<std::path::Path>) -> Result<Self, StoreConfigError> {
        if root.as_ref().as_os_str().is_empty() {
            return Err(StoreConfigError::Invalid("DOCUMENT_STORAGE_ROOT"));
        }
        std::fs::create_dir_all(root.as_ref())
            .map_err(|error| StoreConfigError::Initialization(error.to_string()))?;
        Ok(Self {
            inner: Arc::new(object_store::local::LocalFileSystem::new_with_prefix(root)?),
        })
    }

    pub fn memory() -> Self {
        Self {
            inner: Arc::new(object_store::memory::InMemory::new()),
        }
    }

    pub fn s3(
        endpoint: &str,
        bucket: &str,
        region: &str,
        access_key_id: &str,
        secret_access_key: &str,
    ) -> Result<Self, StoreConfigError> {
        let endpoint_url = url::Url::parse(endpoint)
            .map_err(|_| StoreConfigError::Invalid("DOCUMENT_STORAGE_ENDPOINT"))?;
        if endpoint_url.scheme() != "https"
            || endpoint_url.host_str().is_none()
            || !endpoint_url.username().is_empty()
            || endpoint_url.password().is_some()
            || bucket.trim().is_empty()
            || region.trim().is_empty()
            || access_key_id.trim().is_empty()
            || secret_access_key.trim().is_empty()
        {
            return Err(StoreConfigError::Invalid("S3 document storage settings"));
        }
        let store = object_store::aws::AmazonS3Builder::new()
            .with_endpoint(endpoint)
            .with_bucket_name(bucket)
            .with_region(region)
            .with_access_key_id(access_key_id)
            .with_secret_access_key(secret_access_key)
            .build()?;
        Ok(Self {
            inner: Arc::new(store),
        })
    }
}

#[derive(Clone)]
pub struct DocumentStore {
    inner: Arc<dyn ObjectStoreTrait>,
    max_bytes: usize,
}

impl std::fmt::Debug for DocumentStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DocumentStore")
            .finish_non_exhaustive()
    }
}

impl DocumentStore {
    pub fn from_config(config: &DocumentConfig) -> Result<Self, StoreConfigError> {
        let store = match &config.backend {
            DocumentStorageBackend::Local { root } => StoreConfig::local(root)?,
            DocumentStorageBackend::S3 {
                endpoint,
                bucket,
                region,
                access_key_id,
                secret_access_key,
            } => StoreConfig::s3(
                endpoint,
                bucket,
                region,
                access_key_id,
                secret_access_key.expose_secret(),
            )?,
        };
        Ok(Self::new(store, config.max_bytes))
    }

    pub fn from_object_store(store: Arc<dyn ObjectStoreTrait>) -> Self {
        Self {
            inner: store,
            max_bytes: DEFAULT_MAX_DOCUMENT_BYTES,
        }
    }

    pub fn new(config: StoreConfig, max_bytes: usize) -> Self {
        Self {
            inner: config.inner,
            max_bytes: max_bytes.max(1),
        }
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub fn memory() -> Self {
        Self::new(StoreConfig::memory(), DEFAULT_MAX_DOCUMENT_BYTES)
    }

    #[cfg(test)]
    pub async fn put(&self, key: &str, bytes: Bytes) -> Result<(), DocumentStoreError> {
        validate_object_key(key)?;
        if bytes.len() > self.max_bytes {
            return Err(DocumentStoreError::TooLarge {
                actual: bytes.len(),
                maximum: self.max_bytes,
            });
        }
        self.inner
            .put(&Path::from(key), PutPayload::from_bytes(bytes))
            .await
            .map(|_| ())
            .map_err(DocumentStoreError::from)
    }

    pub async fn get(&self, key: &str) -> Result<GetResult, DocumentStoreError> {
        validate_object_key(key)?;
        self.inner
            .get(&Path::from(key))
            .await
            .map_err(DocumentStoreError::from)
    }

    #[cfg(test)]
    pub async fn get_bytes(&self, key: &str) -> Result<Bytes, DocumentStoreError> {
        self.get(key)
            .await?
            .bytes()
            .await
            .map_err(DocumentStoreError::from)
    }

    pub async fn read_to_writer<W>(
        &self,
        key: &str,
        writer: &mut W,
    ) -> Result<StoredObject, DocumentStoreError>
    where
        W: tokio::io::AsyncWrite + Unpin + Send,
    {
        let mut stream = self.get(key).await?.into_stream();
        let mut hasher = Sha256::new();
        let mut byte_size = 0usize;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            byte_size = byte_size.saturating_add(chunk.len());
            if byte_size > self.max_bytes {
                return Err(DocumentStoreError::TooLarge {
                    actual: byte_size,
                    maximum: self.max_bytes,
                });
            }
            hasher.update(&chunk);
            writer
                .write_all(&chunk)
                .await
                .map_err(|error| DocumentStoreError::Stream(error.to_string()))?;
        }
        writer
            .flush()
            .await
            .map_err(|error| DocumentStoreError::Stream(error.to_string()))?;
        Ok(StoredObject {
            byte_size,
            sha256: hex_digest(hasher.finalize().as_slice()),
            opaque_id: key.to_owned(),
        })
    }

    pub async fn put_stream<S, E>(&self, stream: S) -> Result<StoredObject, DocumentStoreError>
    where
        S: Stream<Item = Result<Bytes, E>>,
        E: std::fmt::Display,
    {
        let key = format!("documents/{}", Uuid::new_v4());
        let mut upload = self.inner.put_multipart(&Path::from(key.as_str())).await?;
        const PART_BYTES: usize = 5 * 1024 * 1024;
        let mut pending = BytesMut::with_capacity(PART_BYTES.min(self.max_bytes));
        let mut hasher = Sha256::new();
        let mut byte_size = 0usize;
        let mut stream = Box::pin(stream);
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    let _ = upload.abort().await;
                    return Err(DocumentStoreError::Stream(error.to_string()));
                }
            };
            byte_size = byte_size.saturating_add(chunk.len());
            if byte_size > self.max_bytes {
                let _ = upload.abort().await;
                return Err(DocumentStoreError::TooLarge {
                    actual: byte_size,
                    maximum: self.max_bytes,
                });
            }
            hasher.update(&chunk);
            let mut chunk = chunk;
            while chunk.has_remaining() {
                let take = (PART_BYTES - pending.len()).min(chunk.remaining());
                pending.extend_from_slice(&chunk[..take]);
                chunk.advance(take);
                if pending.len() == PART_BYTES {
                    let part = pending.split().freeze();
                    if let Err(error) = upload.put_part(PutPayload::from_bytes(part)).await {
                        let _ = upload.abort().await;
                        return Err(DocumentStoreError::from(error));
                    }
                }
            }
        }
        if !pending.is_empty()
            && let Err(error) = upload
                .put_part(PutPayload::from_bytes(pending.freeze()))
                .await
        {
            let _ = upload.abort().await;
            return Err(DocumentStoreError::from(error));
        }
        if let Err(error) = upload.complete().await {
            let _ = upload.abort().await;
            return Err(DocumentStoreError::from(error));
        }
        Ok(StoredObject {
            byte_size,
            sha256: hex_digest(hasher.finalize().as_slice()),
            opaque_id: key,
        })
    }

    pub async fn delete(&self, key: &str) -> Result<(), DocumentStoreError> {
        validate_object_key(key)?;
        self.inner
            .delete(&Path::from(key))
            .await
            .map_err(DocumentStoreError::from)
    }

    /// Stores `bytes` at `key`, replacing any object already there, and
    /// returns the size and SHA-256 of what was written. Callers pass their own
    /// `max_bytes`: workflow files are bounded by their table, not by the PDF
    /// upload limit.
    pub async fn put_object(
        &self,
        key: &ObjectKey,
        bytes: Bytes,
        max_bytes: usize,
    ) -> Result<StoredObject, DocumentStoreError> {
        if bytes.len() > max_bytes {
            return Err(DocumentStoreError::TooLarge {
                actual: bytes.len(),
                maximum: max_bytes,
            });
        }
        let stored = StoredObject {
            byte_size: bytes.len(),
            sha256: hex_digest(Sha256::digest(&bytes).as_slice()),
            opaque_id: key.as_str().to_owned(),
        };
        self.inner
            .put(&Path::from(key.as_str()), PutPayload::from_bytes(bytes))
            .await?;
        Ok(stored)
    }

    /// Reads the whole object, refusing one larger than `max_bytes` before
    /// downloading it.
    pub async fn get_object(
        &self,
        key: &ObjectKey,
        max_bytes: usize,
    ) -> Result<Bytes, DocumentStoreError> {
        let result = self.inner.get(&Path::from(key.as_str())).await?;
        let size = usize::try_from(result.meta.size).unwrap_or(usize::MAX);
        if size > max_bytes {
            return Err(DocumentStoreError::TooLarge {
                actual: size,
                maximum: max_bytes,
            });
        }
        Ok(result.bytes().await?)
    }

    pub async fn delete_object(&self, key: &ObjectKey) -> Result<(), DocumentStoreError> {
        self.inner.delete(&Path::from(key.as_str())).await?;
        Ok(())
    }

    /// Reports whether the object is still stored. A missing object is
    /// `Ok(false)`; any other store failure stays an error.
    pub async fn exists(&self, key: &str) -> Result<bool, DocumentStoreError> {
        validate_object_key(key)?;
        match self.inner.head(&Path::from(key)).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(error) => Err(DocumentStoreError::from(error)),
        }
    }
}

/// A validated object-store key. Every shape is built from stable identifiers,
/// never from a user-provided filename, so a key reveals nothing a caller typed
/// and cannot escape its prefix.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectKey(String);

impl ObjectKey {
    pub fn document(id: Uuid) -> Self {
        Self(format!("documents/{id}"))
    }

    pub fn workflow_file(project_id: Uuid, run_id: Uuid, file_id: Uuid) -> Self {
        Self(format!(
            "projects/{project_id}/workflow-runs/{run_id}/files/{file_id}"
        ))
    }

    /// Accepts only the key shapes this module builds.
    pub fn parse(key: &str) -> Result<Self, DocumentStoreError> {
        let segments: Vec<&str> = key.split('/').collect();
        let is_uuid = |segment: &str| {
            Uuid::parse_str(segment).is_ok_and(|id| id.hyphenated().to_string() == segment)
        };
        let valid = match segments.as_slice() {
            ["documents", id] => is_uuid(id),
            ["projects", project, "workflow-runs", run, "files", file] => {
                is_uuid(project) && is_uuid(run) && is_uuid(file)
            }
            _ => false,
        };
        if valid {
            Ok(Self(key.to_owned()))
        } else {
            Err(DocumentStoreError::InvalidKey)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable code for a report whose stored PDF bytes are no longer in document
/// storage. The API returns it for reparse requests and the worker records it
/// in `parser_error`, so the UI can ask for the PDF to be uploaded again.
pub const DOCUMENT_BLOB_MISSING_CODE: &str = "document_blob_missing";

#[derive(Debug, Error)]
pub enum DocumentStoreError {
    #[error("document object store operation failed: {0}")]
    Store(#[from] object_store::Error),
    #[error("document is {actual} bytes; maximum is {maximum} bytes")]
    TooLarge { actual: usize, maximum: usize },
    #[error("document stream failed: {0}")]
    Stream(String),
    #[error("document object key is invalid")]
    InvalidKey,
}

impl DocumentStoreError {
    /// True when the store reports that the object itself does not exist.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Store(object_store::Error::NotFound { .. }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredObject {
    pub byte_size: usize,
    pub sha256: String,
    pub opaque_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedBlock {
    pub page_number: u32,
    pub page_width: f32,
    pub page_height: f32,
    pub ordinal: u32,
    pub kind: String,
    pub text: String,
    pub bbox: Option<deepref_domain::NormalizedBoundingBox>,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedPage {
    pub page_number: u32,
    pub width: f32,
    pub height: f32,
    pub ocr_required: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedDocument {
    pub pages: Vec<ParsedPage>,
    pub blocks: Vec<ParsedBlock>,
    pub ocr_required: bool,
}

/// Structure derived for one [`ParsedBlock`] (same index as `ParsedDocument::blocks`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BlockStructure {
    /// `text`, `heading`, `caption`, `reference`, `title` or `front_matter` (mirrors `kind`).
    pub role: String,
    /// Heading hierarchy the block sits under, e.g. `["Methods", "Participants"]`. A heading
    /// block's own path ends with its title.
    pub section_path: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedReference {
    pub page_number: u32,
    pub raw_text: String,
    /// Lower-cased DOI when one appears in the entry.
    pub doi: Option<String>,
    pub year: Option<u16>,
}

/// A parsed document plus the structure the native pipeline derived from font statistics.
#[derive(Debug, Clone, PartialEq)]
pub struct StructuredDocument {
    pub document: ParsedDocument,
    /// Parallel to `document.blocks` (index == position == `ordinal`).
    pub block_structure: Vec<BlockStructure>,
    pub references: Vec<ParsedReference>,
}

impl StructuredDocument {
    pub fn unstructured(document: ParsedDocument) -> Self {
        let block_structure = document
            .blocks
            .iter()
            .map(|block| BlockStructure {
                role: block.kind.clone(),
                section_path: Vec::new(),
            })
            .collect();
        Self {
            document,
            block_structure,
            references: Vec::new(),
        }
    }
}

pub fn content_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_digest(hasher.finalize().as_slice())
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn validate_object_key(key: &str) -> Result<(), DocumentStoreError> {
    ObjectKey::parse(key).map(|_| ())
}

fn storage_key(id: deepref_application::DocumentStorageId) -> String {
    format!("documents/{}", id.as_uuid())
}

impl deepref_application::DocumentStore for DocumentStore {
    fn write_pdf<'a>(
        &'a self,
        content: deepref_application::DocumentByteStream<'a>,
    ) -> deepref_application::DocumentFuture<'a, deepref_application::StoredDocumentContent> {
        Box::pin(async move {
            let stored = self.put_stream(content).await.map_err(port_error)?;
            let id = stored
                .opaque_id
                .strip_prefix("documents/")
                .and_then(|value| Uuid::parse_str(value).ok())
                .ok_or_else(|| {
                    deepref_application::DocumentPortError::Operation(
                        "storage returned a non-opaque document identity".to_owned(),
                    )
                })?;
            Ok(deepref_application::StoredDocumentContent {
                storage_id: deepref_application::DocumentStorageId::new(id),
                byte_size: stored.byte_size as u64,
                sha256: stored.sha256,
            })
        })
    }

    fn read_pdf<'a>(
        &'a self,
        id: deepref_application::DocumentStorageId,
    ) -> deepref_application::DocumentFuture<'a, deepref_application::DocumentByteStream<'a>> {
        Box::pin(async move {
            let result = self.get(&storage_key(id)).await.map_err(port_error)?;
            let stream = result.into_stream().map(|chunk| {
                chunk.map_err(|error| {
                    deepref_application::DocumentPortError::Operation(error.to_string())
                })
            });
            Ok(Box::pin(stream) as deepref_application::DocumentByteStream<'a>)
        })
    }

    fn delete_pdf<'a>(
        &'a self,
        id: deepref_application::DocumentStorageId,
    ) -> deepref_application::DocumentFuture<'a, ()> {
        Box::pin(async move { self.delete(&storage_key(id)).await.map_err(port_error) })
    }
}

fn port_error(error: DocumentStoreError) -> deepref_application::DocumentPortError {
    match error {
        DocumentStoreError::TooLarge { .. } => deepref_application::DocumentPortError::TooLarge,
        other => deepref_application::DocumentPortError::Operation(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;

    #[tokio::test]
    async fn exists_distinguishes_missing_objects_from_stored_ones() {
        let store = DocumentStore::memory();
        let key = "documents/00000000-0000-0000-0000-000000000002";
        assert!(
            !store
                .exists(key)
                .await
                .expect("a missing object is not a store error")
        );
        store
            .put(key, Bytes::from_static(b"%PDF-test"))
            .await
            .expect("put should succeed");
        assert!(store.exists(key).await.expect("exists should succeed"));
    }

    #[test]
    fn object_keys_are_built_from_identifiers_and_reject_anything_else() {
        let (project, run, file) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let key = ObjectKey::workflow_file(project, run, file);
        assert_eq!(
            key.as_str(),
            format!("projects/{project}/workflow-runs/{run}/files/{file}")
        );
        assert_eq!(ObjectKey::parse(key.as_str()).unwrap(), key);
        let document = ObjectKey::document(file);
        assert_eq!(ObjectKey::parse(document.as_str()).unwrap(), document);
        for invalid in [
            "",
            "documents/report.pdf",
            "documents/../etc/passwd",
            &format!("projects/{project}/workflow-runs/{run}/files/export.csv"),
            &format!("projects/{project}/workflow-runs/{run}/files/{file}/extra"),
            &format!(
                "projects/{project}/workflow-runs/{run}/files/{}",
                file.as_simple()
            ),
            &format!(
                "projects/{project}/workflow-runs/{run}/files/{}",
                file.to_string().to_uppercase()
            ),
            &format!("projects/{project}/other/{run}/files/{file}"),
        ] {
            assert!(
                matches!(
                    ObjectKey::parse(invalid),
                    Err(DocumentStoreError::InvalidKey)
                ),
                "{invalid} must be rejected"
            );
        }
    }

    #[tokio::test]
    async fn put_object_reports_what_it_wrote_and_get_object_bounds_reads() {
        let store = DocumentStore::memory();
        let key = ObjectKey::workflow_file(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let stored = store
            .put_object(&key, Bytes::from_static(b"id,title\n"), 1024)
            .await
            .expect("object should be written");
        assert_eq!(stored.byte_size, 9);
        assert_eq!(stored.sha256, content_sha256(b"id,title\n"));
        assert_eq!(stored.opaque_id, key.as_str());
        assert_eq!(
            store.get_object(&key, 1024).await.unwrap(),
            Bytes::from_static(b"id,title\n")
        );
        assert!(matches!(
            store.get_object(&key, 4).await,
            Err(DocumentStoreError::TooLarge {
                actual: 9,
                maximum: 4
            })
        ));
        assert!(matches!(
            store
                .put_object(&key, Bytes::from_static(b"12345"), 4)
                .await,
            Err(DocumentStoreError::TooLarge { .. })
        ));
        store.delete_object(&key).await.expect("delete");
        assert!(
            store
                .get_object(&key, 1024)
                .await
                .unwrap_err()
                .is_not_found()
        );
    }

    #[tokio::test]
    async fn memory_store_round_trips_content() {
        let store = DocumentStore::memory();
        store
            .put(
                "documents/00000000-0000-0000-0000-000000000001",
                Bytes::from_static(b"%PDF-test"),
            )
            .await
            .expect("put should succeed");
        let content = store
            .get("documents/00000000-0000-0000-0000-000000000001")
            .await
            .expect("get should succeed")
            .bytes()
            .await
            .expect("bytes should succeed");
        assert_eq!(content, Bytes::from_static(b"%PDF-test"));
    }

    #[tokio::test]
    async fn streaming_write_hashes_bounds_reads_and_deletes_without_partial_objects() {
        let store = DocumentStore::new(StoreConfig::memory(), 10);
        let stored = store
            .put_stream(stream::iter([
                Ok::<_, String>(Bytes::from_static(b"%PDF-")),
                Ok(Bytes::from_static(b"12345")),
            ]))
            .await
            .expect("bounded stream should persist");
        assert_eq!(stored.byte_size, 10);
        assert_eq!(stored.sha256, content_sha256(b"%PDF-12345"));
        assert!(stored.opaque_id.starts_with("documents/"));
        assert!(!stored.opaque_id.contains(".pdf"));

        let mut sink = tokio::io::sink();
        let read = store
            .read_to_writer(&stored.opaque_id, &mut sink)
            .await
            .expect("streaming read should succeed");
        assert_eq!(read.sha256, stored.sha256);
        store
            .delete(&stored.opaque_id)
            .await
            .expect("delete should succeed");
        assert!(store.get(&stored.opaque_id).await.is_err());

        let too_large = store
            .put_stream(stream::iter([
                Ok::<_, String>(Bytes::from_static(b"123456")),
                Ok(Bytes::from_static(b"78901")),
            ]))
            .await;
        assert!(matches!(
            too_large,
            Err(DocumentStoreError::TooLarge { .. })
        ));
        let objects = store.inner.list(None).collect::<Vec<_>>().await;
        assert!(objects.is_empty(), "failed writes must not leave an object");

        let upstream_error = store
            .put_stream(stream::iter([
                Ok(Bytes::from_static(b"%PDF-")),
                Err("upstream disconnected"),
            ]))
            .await;
        assert!(matches!(upstream_error, Err(DocumentStoreError::Stream(_))));
        let objects = store.inner.list(None).collect::<Vec<_>>().await;
        assert!(objects.is_empty(), "stream failures must abort the object");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn store_write_failure_does_not_publish_a_partial_object() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("deepref-store-failure-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let store = DocumentStore::new(StoreConfig::local(&root).unwrap(), 1024);
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500)).unwrap();
        let result = store
            .put_stream(stream::iter([Ok::<_, String>(Bytes::from_static(
                b"%PDF-test",
            ))]))
            .await;
        assert!(result.is_err());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::remove_dir(&root).unwrap();
    }

    #[tokio::test]
    async fn object_keys_are_opaque_and_reject_traversal() {
        let store = DocumentStore::memory();
        for key in [
            "../secret",
            "documents/../secret",
            "documents/a/b",
            "uploads/name.pdf",
        ] {
            assert!(matches!(
                store.get(key).await,
                Err(DocumentStoreError::InvalidKey)
            ));
            assert!(matches!(
                store.delete(key).await,
                Err(DocumentStoreError::InvalidKey)
            ));
        }
    }

    #[test]
    fn s3_configuration_requires_https_and_complete_credentials() {
        assert!(StoreConfig::s3("http://s3.example", "bucket", "region", "key", "secret").is_err());
        assert!(StoreConfig::s3("https://s3.example", "", "region", "key", "secret").is_err());
        assert!(StoreConfig::s3("https://s3.example", "bucket", "region", "key", "secret").is_ok());
    }

    #[test]
    fn configured_local_store_uses_injected_source() {
        let root =
            std::env::temp_dir().join(format!("deepref-configured-store-{}", Uuid::new_v4()));
        let config = DocumentConfig::from_source(&|name| match name {
            "APP_ENV" => Some("development".into()),
            "DOCUMENT_STORAGE_ROOT" => Some(root.to_string_lossy().into_owned()),
            "DOCUMENT_MAX_BYTES" => Some("1024".into()),
            _ => None,
        })
        .unwrap();
        let store = DocumentStore::from_config(&config).unwrap();
        assert_eq!(store.max_bytes(), 1024);
        std::fs::remove_dir_all(root).unwrap();
    }
}
