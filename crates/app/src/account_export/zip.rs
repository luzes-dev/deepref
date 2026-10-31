//! Disk-backed ZIP packaging, with bounded chunk I/O and private RAII staging.
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};
use bytes::Bytes;
use deepref_documents::DocumentStore;
use deepref_domain::UserId;
use futures::{Stream, StreamExt};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::collector::{Inventory, Source};
use super::generation::GenerationBudget;

pub struct Staging(PathBuf);
impl Staging {
    pub fn new() -> std::io::Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("deepref-export-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()?;
        // Staging takes cleanup ownership so failures retain the existing warning.
        Ok(Self(directory.keep()))
    }
    pub(super) fn for_attempt(attempt: uuid::Uuid) -> std::io::Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix(&format!("deepref-export-{attempt}-"))
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()?;
        Ok(Self(directory.keep()))
    }
    pub(super) fn cleanup(self) -> std::io::Result<()> {
        std::fs::remove_dir_all(&self.0)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, "export staging cleanup failed");
        }
    }
}

pub struct ArchiveSummary {
    pub path: PathBuf,
    pub total_bytes: u64,
    pub sha256_hex: String,
    pub manifest: serde_json::Value,
}

pub fn file_stream(file: tokio::fs::File) -> impl Stream<Item = std::io::Result<Bytes>> + Send {
    futures::stream::try_unfold(file, |mut file| async move {
        let mut buffer = vec![0u8; 64 * 1024];
        let size = file.read(&mut buffer).await?;
        if size == 0 {
            return Ok(None);
        }
        buffer.truncate(size);
        Ok(Some((Bytes::from(buffer), file)))
    })
}

pub(super) async fn package(
    store: &DocumentStore,
    inventory: &Inventory,
    dir: &Path,
    user: UserId,
    budget: &GenerationBudget,
) -> anyhow::Result<ArchiveSummary> {
    budget.check()?;
    let path = dir.join("export.zip");
    let mut archive = ZipWriter::new(
        File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?,
    );
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o600);
    let mut written = 0u64;
    for (name, source) in &inventory.json_files {
        budget.check()?;
        archive.start_file(name, options)?;
        let file = tokio::fs::File::open(source).await?;
        let stream = file_stream(file);
        futures::pin_mut!(stream);
        while let Some(chunk) = stream.next().await {
            budget.check()?;
            let chunk = chunk?;
            charge(&mut written, chunk.len())?;
            archive.write_all(&chunk)?;
        }
    }
    for blob in &inventory.blobs {
        budget.check()?;
        archive.start_file(&blob.archive_path, options)?;
        let stream: std::pin::Pin<Box<dyn Stream<Item = anyhow::Result<Bytes>> + Send>> =
            match &blob.source {
                Source::Object { key, .. } => Box::pin(
                    store
                        .get(key.as_str())
                        .await?
                        .into_stream()
                        .map(|item| item.map_err(Into::into)),
                ),
                Source::Inline(path) => Box::pin(
                    file_stream(tokio::fs::File::open(path).await?)
                        .map(|item| item.map_err(Into::into)),
                ),
            };
        let mut stream = stream;
        let mut actual = 0u64;
        let mut hasher = Sha256::new();
        while let Some(chunk) = stream.next().await {
            budget.check()?;
            let chunk = chunk?;
            actual = actual
                .checked_add(u64::try_from(chunk.len())?)
                .context("source byte count overflow")?;
            ensure!(
                actual <= blob.byte_size,
                "source length exceeds declared size"
            );
            charge(&mut written, chunk.len())?;
            hasher.update(&chunk);
            archive.write_all(&chunk)?;
        }
        ensure!(actual == blob.byte_size, "source length mismatch");
        ensure!(
            hex_digest(hasher.finalize().as_slice()) == blob.sha256,
            "source SHA256 mismatch"
        );
    }
    // Completion means the declared inventory was packaged, not that every
    // product table is exported. The manifest records that distinction.
    let manifest = serde_json::json!({
        "version": 2, "user_id": user.as_uuid(), "generated_at": chrono::Utc::now(),
        "is_partial": true,
        "partial_reasons": ["This export covers the named scientific sections and original document/workflow files. Credentials, executable workflow state, raw provider envelopes and internal operational data are excluded."],
        "included_workspaces": inventory.workspaces,
        "json_sections": inventory.json_files.iter().map(|(name,_)|name).collect::<Vec<_>>(),
        "total_source_files": inventory.blobs.len(),
        "exclusions": ["session cookies and access/refresh tokens", "WorkOS identity IDs", "API key secrets and verifiers", "raw connector/provider payloads", "administrative/security audit", "internal financial retention notes", "workflow secrets and executable workflow configuration", "reconstructable embeddings and graph caches", "workflow node input/output and trigger bodies", "internal execution errors and prepared model requests", "non-owned workspaces"],
        "limits": {"rows": super::collector::MAX_ROWS,"files":super::collector::MAX_FILES,"json_bytes":super::collector::MAX_JSON_BYTES,"source_bytes":super::collector::MAX_SOURCE_BYTES,"seconds":600}
    });
    archive.start_file("manifest.json", options)?;
    let body = serde_json::to_vec(&manifest)?;
    charge(&mut written, body.len())?;
    archive.write_all(&body)?;
    budget.check()?;
    let mut file = archive.finish()?;
    budget.check()?;
    file.flush()?;
    budget.check()?;
    let total_bytes = file.metadata()?.len();
    ensure!(
        total_bytes <= super::MAX_ARCHIVE_BYTES,
        "archive budget exceeded"
    );
    file.rewind()?;
    let sha256_hex = hash_archive(&mut file, budget)?;
    Ok(ArchiveSummary {
        path,
        total_bytes,
        sha256_hex,
        manifest,
    })
}

pub(super) fn hash_archive(
    reader: &mut impl Read,
    budget: &GenerationBudget,
) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        budget.check()?;
        let count = reader.read(&mut buffer)?;
        budget.check()?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex_digest(hasher.finalize().as_slice()))
}
fn charge(written: &mut u64, bytes: usize) -> anyhow::Result<()> {
    *written = written
        .checked_add(u64::try_from(bytes)?)
        .context("archive size overflow")?;
    ensure!(
        *written <= super::MAX_ARCHIVE_BYTES,
        "archive byte budget exceeded"
    );
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
