//! Explicit maintenance cutover. Ordinary migrations never discard the
//! compatibility column. Deployment drain attestations are operational facts;
//! the command verifies database/object preconditions itself.
use deepref_documents::{DocumentStore, ObjectKey, content_sha256};
use futures::TryStreamExt;
use sqlx::{PgPool, Row};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowFileCutover {
    ObjectOnly,
    Contract,
}

impl WorkflowFileCutover {
    pub const fn drain_attestation(self) -> &'static str {
        match self {
            Self::ObjectOnly => "all-legacy-readers-and-writers-drained",
            Self::Contract => "all-content-dependent-binaries-drained-subsequent-release",
        }
    }
}

/// Run only in a maintenance window after the named fleet has actually drained.
/// Locks writers before checking every object. Failure preserves schema and
/// state atomically. After object-only, legacy writers fail closed at the DB.
pub async fn cutover_workflow_file_storage(
    pool: &PgPool,
    store: &DocumentStore,
    target: WorkflowFileCutover,
    attestation: &str,
) -> anyhow::Result<u64> {
    anyhow::ensure!(
        attestation == target.drain_attestation(),
        "exact drain attestation required: {}",
        target.drain_attestation()
    );
    let mut tx = pool.begin().await?;
    // Acquire the table lock before the state lock: writers/triggers already
    // hold a table lock before taking their shared state lock.
    sqlx::query("LOCK TABLE workflow_files IN ACCESS EXCLUSIVE MODE NOWAIT")
        .execute(&mut *tx)
        .await?;
    let mode: String = sqlx::query_scalar(
        "SELECT mode FROM workflow_file_storage_state WHERE singleton FOR UPDATE",
    )
    .fetch_one(&mut *tx)
    .await?;
    let (required, next) = match target {
        WorkflowFileCutover::ObjectOnly => ("overlap", "object_only"),
        WorkflowFileCutover::Contract => ("object_only", "contracted"),
    };
    anyhow::ensure!(
        mode == required,
        "cutover requires {required}, current mode is {mode}"
    );
    let mut verified = 0;
    {
        let mut rows = sqlx::query(
            "SELECT id,storage_key,content_sha256,byte_size FROM workflow_files ORDER BY id",
        )
        .fetch(&mut *tx);
        while let Some(row) = rows.try_next().await? {
            let id: uuid::Uuid = row.try_get("id")?;
            let key: Option<String> = row.try_get("storage_key")?;
            let hash: Option<String> = row.try_get("content_sha256")?;
            let size: Option<i64> = row.try_get("byte_size")?;
            let (Some(key), Some(hash), Some(size)) = (key, hash, size) else {
                anyhow::bail!("workflow file {id} still needs verified backfill");
            };
            let key = ObjectKey::parse(&key)?;
            let bytes = store
                .get_object(&key, crate::MAX_WORKFLOW_FILE_BYTES)
                .await?;
            anyhow::ensure!(
                i64::try_from(bytes.len()) == Ok(size) && content_sha256(&bytes) == hash,
                "workflow file {id} object integrity mismatch"
            );
            verified += 1;
        }
    }
    match target {
        WorkflowFileCutover::ObjectOnly => {
            sqlx::query("ALTER TABLE workflow_files ALTER COLUMN content DROP NOT NULL")
                .execute(&mut *tx)
                .await?;
        }
        WorkflowFileCutover::Contract => {
            sqlx::query("ALTER TABLE workflow_files DROP COLUMN content")
                .execute(&mut *tx)
                .await?;
        }
    }
    sqlx::query("UPDATE workflow_file_storage_state SET mode=$1,changed_at=now() WHERE singleton")
        .bind(next)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(verified)
}
