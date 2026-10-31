//! Files produced by workflow export blocks.
//!
//! The bytes live in object storage under a key built from stable identifiers.
//! `workflow_files` keeps the reference (key, SHA-256, size) and, during the
//! compatibility overlap, a copy of the bytes in `content` so binaries that
//! predate the reference columns keep working. Writers store the object before
//! the row exists; readers prefer the object, check it against the recorded
//! hash and fall back to `content`. The backfill moves older rows over.

use bytes::Bytes;
use deepref_documents::{DocumentStore, DocumentStoreError, ObjectKey, content_sha256};
use deepref_domain::{JobKind, ProjectId};
use serde_json::json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::object_lifecycle::{ObjectOwner, begin_object_upload};
use crate::workflows::WorkflowError;

/// Largest file an export block may store; the same bound `workflow_files`
/// enforces on `content`.
pub const MAX_WORKFLOW_FILE_BYTES: usize = 16 * 1024 * 1024;

/// Files one backfill job moves before it hands the rest to a follow-up job.
pub const WORKFLOW_FILE_BACKFILL_BATCH: i64 = 200;

/// A stored export file.
#[derive(Debug, Clone)]
pub struct WorkflowFile {
    pub id: Uuid,
    pub name: String,
    pub content_type: String,
    pub content: Vec<u8>,
    pub source: WorkflowFileSource,
}

/// Where a read got its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowFileSource {
    /// The verified object.
    Object,
    /// The row has no object reference yet, so `content` was read.
    Database,
    /// The row has an object reference that could not be used, so `content`
    /// was read instead. Each case is also logged.
    DatabaseFallback(WorkflowFileFallback),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowFileFallback {
    ObjectMissing,
    /// The object's bytes differ from the recorded hash or size.
    IntegrityMismatch,
    StoreUnavailable,
    /// The stored key is not one this application builds.
    InvalidReference,
}

fn storage_error(error: DocumentStoreError) -> WorkflowError {
    match error {
        DocumentStoreError::TooLarge { maximum, .. } => {
            WorkflowError::InvalidInput(format!("a workflow file may not exceed {maximum} bytes"))
        }
        other => WorkflowError::ObjectStorage(other),
    }
}

/// A file an export block is about to store.
#[derive(Debug, Clone, Copy)]
pub struct NewWorkflowFile<'a> {
    pub project_id: Uuid,
    pub run_id: Uuid,
    pub node_id: &'a str,
    /// Display and download name only; it never reaches the object key.
    pub name: &'a str,
    pub content_type: &'a str,
    pub content: &'a [u8],
}

/// Store a file for an export block. The object is written first; the row,
/// which carries the reference and still the bytes, is inserted afterwards. A
/// failed or ambiguous commit leaves a durable cleanup intent, and cleanup
/// rechecks references before deleting any bytes.
pub async fn create_workflow_file(
    pool: &PgPool,
    store: &DocumentStore,
    file: NewWorkflowFile<'_>,
) -> Result<Uuid, WorkflowError> {
    let file_id = Uuid::new_v4();
    let key = crate::object_lifecycle::workflow_file_key(
        pool,
        ProjectId::new(file.project_id),
        file.run_id,
        file_id,
    )
    .await
    .map_err(|error| WorkflowError::InvalidStoredValue(error.to_string()))?;
    let mut tx = begin_object_upload(
        pool,
        ObjectOwner::Project(ProjectId::new(file.project_id)),
        &key,
    )
    .await
    .map_err(|error| WorkflowError::InvalidStoredValue(error.to_string()))?;
    sqlx::query("LOCK TABLE workflow_files IN ROW EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await?;
    let mode: String = sqlx::query_scalar(
        "SELECT mode FROM workflow_file_storage_state WHERE singleton FOR SHARE",
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("SELECT set_config('deepref.workflow_file_writer','object-v1',true)")
        .execute(&mut *tx)
        .await?;
    let stored = store
        .put_object(
            &key,
            Bytes::copy_from_slice(file.content),
            MAX_WORKFLOW_FILE_BYTES,
        )
        .await
        .map_err(storage_error)?;
    let insert = if mode == "contracted" {
        "INSERT INTO workflow_files
           (id, project_id, run_id, node_id, name, content_type, storage_key, content_sha256, byte_size)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)"
    } else {
        "INSERT INTO workflow_files
           (id, project_id, run_id, node_id, name, content_type, content,
            storage_key, content_sha256, byte_size)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)"
    };
    let query = sqlx::query(insert)
        .bind(file_id)
        .bind(file.project_id)
        .bind(file.run_id)
        .bind(file.node_id)
        .bind(file.name)
        .bind(file.content_type);
    let query = if mode == "contracted" {
        query
    } else {
        query.bind(if mode == "overlap" {
            Some(file.content)
        } else {
            None
        })
    };
    let inserted = query
        .bind(key.as_str())
        .bind(&stored.sha256)
        .bind(stored.byte_size as i64)
        .execute(&mut *tx)
        .await;
    if let Err(error) = inserted {
        return Err(error.into());
    }
    tx.commit().await?;
    Ok(file_id)
}

/// Read a file, preferring its object. An object that is missing, unreadable
/// or does not match the recorded hash and size is reported in the log and
/// replaced by the bytes still held in `content`.
pub async fn get_workflow_file(
    pool: &PgPool,
    store: &DocumentStore,
    project_id: ProjectId,
    file_id: Uuid,
) -> Result<WorkflowFile, WorkflowError> {
    let row = sqlx::query(
        "SELECT id, name, content_type, storage_key, content_sha256, byte_size
         FROM workflow_files WHERE project_id=$1 AND id=$2",
    )
    .bind(project_id.as_uuid())
    .bind(file_id)
    .fetch_optional(pool)
    .await?
    .ok_or(WorkflowError::NotFound)?;
    let storage_key: Option<String> = row.get("storage_key");
    let (content, source) = match storage_key {
        None => (
            load_content(pool, file_id).await?,
            WorkflowFileSource::Database,
        ),
        Some(key) => {
            let expected_sha256: String = row.get("content_sha256");
            let expected_size: i64 = row.get("byte_size");
            match read_verified_object(store, &key, &expected_sha256, expected_size).await {
                Ok(bytes) => (bytes, WorkflowFileSource::Object),
                Err(reason) => {
                    match reason {
                        WorkflowFileFallback::ObjectMissing => tracing::warn!(
                            %file_id, "workflow file object is missing; serving the database copy"
                        ),
                        WorkflowFileFallback::IntegrityMismatch => tracing::error!(
                            %file_id, "workflow file object does not match its recorded hash; serving the database copy"
                        ),
                        WorkflowFileFallback::StoreUnavailable => tracing::error!(
                            %file_id, "workflow file object could not be read; serving the database copy"
                        ),
                        WorkflowFileFallback::InvalidReference => tracing::error!(
                            %file_id, "workflow file has an invalid storage key; serving the database copy"
                        ),
                    }
                    (
                        load_content(pool, file_id).await?,
                        WorkflowFileSource::DatabaseFallback(reason),
                    )
                }
            }
        }
    };
    Ok(WorkflowFile {
        id: row.get("id"),
        name: row.get("name"),
        content_type: row.get("content_type"),
        content,
        source,
    })
}

async fn load_content(pool: &PgPool, file_id: Uuid) -> Result<Vec<u8>, WorkflowError> {
    // to_jsonb keeps this compatibility fallback executable after the explicit
    // contract drops the column. Cast the server's bytea text representation
    // back to bytea, handling both hex and escape bytea_output settings.
    let encoded: Option<String> =
        sqlx::query_scalar("SELECT to_jsonb(f)->>'content' FROM workflow_files f WHERE id=$1")
            .bind(file_id)
            .fetch_optional(pool)
            .await?
            .flatten();
    let Some(encoded) = encoded else {
        return Err(WorkflowError::InvalidStoredValue(
            "workflow file has no compatibility bytes; object recovery is required".to_owned(),
        ));
    };
    let content: Vec<u8> = sqlx::query_scalar("SELECT $1::text::bytea")
        .bind(encoded)
        .fetch_one(pool)
        .await?;
    Ok(content)
}

async fn read_verified_object(
    store: &DocumentStore,
    key: &str,
    expected_sha256: &str,
    expected_size: i64,
) -> Result<Vec<u8>, WorkflowFileFallback> {
    let key = ObjectKey::parse(key).map_err(|_| WorkflowFileFallback::InvalidReference)?;
    let limit = usize::try_from(expected_size)
        .unwrap_or(0)
        .min(MAX_WORKFLOW_FILE_BYTES);
    let bytes = store
        .get_object(&key, limit)
        .await
        .map_err(|error| match error {
            _ if error.is_not_found() => WorkflowFileFallback::ObjectMissing,
            DocumentStoreError::TooLarge { .. } => WorkflowFileFallback::IntegrityMismatch,
            _ => WorkflowFileFallback::StoreUnavailable,
        })?;
    if i64::try_from(bytes.len()) != Ok(expected_size) || content_sha256(&bytes) != expected_sha256
    {
        return Err(WorkflowFileFallback::IntegrityMismatch);
    }
    Ok(bytes.to_vec())
}

/// What one backfill run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkflowFileBackfill {
    pub migrated: u64,
    /// Files that could not be moved this run; they stay readable from
    /// `content` and a later run tries them again.
    pub failed: u64,
    /// Files of the project that still lack an object reference.
    pub remaining: u64,
}

/// Move up to `limit` of a project's files that have no object reference into
/// object storage. Safe to repeat and to interrupt: each attempt reserves a
/// unique durable upload intent, the row is selected by its missing reference, and the
/// reference is only recorded after the written object reads back with the
/// hash of `content`.
pub async fn backfill_workflow_files(
    pool: &PgPool,
    store: &DocumentStore,
    project_id: ProjectId,
    limit: i64,
) -> Result<WorkflowFileBackfill, WorkflowError> {
    let project = project_id.as_uuid();
    let candidates: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, run_id FROM workflow_files
         WHERE project_id=$1 AND storage_key IS NULL
         ORDER BY id LIMIT $2",
    )
    .bind(project)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    let mut report = WorkflowFileBackfill::default();
    for (file_id, run_id) in candidates {
        match backfill_one(pool, store, project, run_id, file_id).await {
            Ok(()) => report.migrated += 1,
            Err(error) => {
                tracing::error!(%file_id, %error, "workflow file backfill failed for one file");
                report.failed += 1;
            }
        }
    }
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow_files WHERE project_id=$1 AND storage_key IS NULL",
    )
    .bind(project)
    .fetch_one(pool)
    .await?;
    report.remaining = u64::try_from(remaining).unwrap_or(0);
    Ok(report)
}

async fn backfill_one(
    pool: &PgPool,
    store: &DocumentStore,
    project: Uuid,
    run_id: Uuid,
    file_id: Uuid,
) -> Result<(), WorkflowError> {
    let key = crate::object_lifecycle::workflow_file_key(
        pool,
        ProjectId::new(project),
        run_id,
        Uuid::new_v4(),
    )
    .await
    .map_err(|error| WorkflowError::InvalidStoredValue(error.to_string()))?;
    let mut tx = begin_object_upload(pool, ObjectOwner::Project(ProjectId::new(project)), &key)
        .await
        .map_err(|error| WorkflowError::InvalidStoredValue(error.to_string()))?;
    sqlx::query("LOCK TABLE workflow_files IN ROW EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await?;
    let content: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT content FROM workflow_files WHERE id=$1 AND storage_key IS NULL FOR UPDATE",
    )
    .bind(file_id)
    .fetch_optional(&mut *tx)
    .await?;
    // Gone, or another run already moved it.
    let Some(content) = content else {
        return Ok(());
    };
    let expected = content_sha256(&content);
    let size = content.len();
    store
        .put_object(&key, Bytes::from(content), MAX_WORKFLOW_FILE_BYTES)
        .await
        .map_err(storage_error)?;
    let written = store
        .get_object(&key, MAX_WORKFLOW_FILE_BYTES)
        .await
        .map_err(storage_error)?;
    if content_sha256(&written) != expected || written.len() != size {
        return Err(WorkflowError::InvalidStoredValue(format!(
            "workflow file {file_id} did not read back from object storage unchanged"
        )));
    }
    sqlx::query("SELECT set_config('deepref.workflow_file_writer','object-v1',true)")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE workflow_files
         SET storage_key=$2, content_sha256=$3, byte_size=$4
         WHERE id=$1 AND storage_key IS NULL",
    )
    .bind(file_id)
    .bind(key.as_str())
    .bind(&expected)
    .bind(size as i64)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Enqueue a backfill job for every project that still has files without an
/// object reference. The job key names the project's first pending file and
/// its pending count, so a repeat call while that job exists enqueues nothing
/// new, and a job that made progress is followed by a fresh one. Returns how
/// many jobs were enqueued or found.
pub async fn schedule_workflow_file_backfills(
    pool: &PgPool,
    max_projects: i64,
) -> anyhow::Result<usize> {
    let pending: Vec<(Uuid, Uuid, i64)> = sqlx::query_as(
        "SELECT project_id, (array_agg(id ORDER BY id))[1], count(*)
         FROM workflow_files WHERE storage_key IS NULL
         GROUP BY project_id ORDER BY project_id LIMIT $1",
    )
    .bind(max_projects)
    .fetch_all(pool)
    .await?;
    for (project, first, count) in &pending {
        crate::enqueue_job_pool(
            pool,
            &crate::job(
                Uuid::new_v4(),
                ProjectId::new(*project),
                JobKind::BackfillWorkflowFiles,
                json!({ "project_id": project }),
                format!("backfill_workflow_files:{project}:{first}:{count}"),
            ),
        )
        .await?;
    }
    Ok(pending.len())
}
