//! Durable blob intent and reference fencing. Business retention decides when
//! metadata may disappear; this module applies operational cleanup afterwards.
use chrono::{DateTime, Utc};
use deepref_application::jobs::JobScope;
use deepref_documents::{DocumentStore, ObjectKey};
use deepref_domain::{JobKind, ProjectId, UserId};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

#[derive(Debug, Clone, Copy)]
pub enum ObjectOwner {
    Project(ProjectId),
    Account(UserId),
}

const LEGACY_DOCUMENT_PATTERN: &str =
    "^documents/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$";
const TENANT_DOCUMENT_PATTERN: &str = "^workspaces/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/projects/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/documents/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$";
pub const TENANT_KEY_DRAIN_ATTESTATION: &str = "all-legacy-key-parsers-drained";

/// The constraint transition and factory flag commit atomically. The exact
/// schema checks prevent accidentally relaxing an unexpected constraint.
pub async fn activate_tenant_object_keys(pool: &PgPool, attestation: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        attestation == TENANT_KEY_DRAIN_ATTESTATION,
        "exact legacy key-parser drain attestation required"
    );
    let mut tx = pool.begin().await?;
    // Producers already hold this lock implicitly on INSERT. Acquire it first
    // and without waiting, so there is no rollout-row/table lock inversion.
    sqlx::query("LOCK TABLE documents IN ACCESS EXCLUSIVE MODE NOWAIT")
        .execute(&mut *tx)
        .await?;
    let activated: bool=sqlx::query_scalar("SELECT tenant_keys_activated FROM object_storage_rollout WHERE singleton FOR UPDATE NOWAIT")
        .fetch_one(&mut *tx).await?;
    let constraints: Vec<(String, bool, String)> = sqlx::query_as(
        "SELECT conname::text,convalidated,pg_get_expr(conbin,conrelid) FROM pg_constraint
         WHERE conrelid='documents'::regclass AND contype='c'
         AND conname IN ('documents_object_key_check','documents_object_key_tenant_layout_check')",
    )
    .fetch_all(&mut *tx)
    .await?;
    let legacy =
        format!("((object_key IS NULL) OR (object_key ~ '{LEGACY_DOCUMENT_PATTERN}'::text))");
    let tenant = format!(
        "((object_key IS NULL) OR (object_key ~ '{LEGACY_DOCUMENT_PATTERN}'::text) OR (object_key ~ '{TENANT_DOCUMENT_PATTERN}'::text))"
    );
    anyhow::ensure!(
        constraints.iter().any(|(name, validated, sql)| name
            == "documents_object_key_tenant_layout_check"
            && *validated
            && sql == &tenant),
        "known widened object-key constraint must be present and validated"
    );
    if !activated {
        anyhow::ensure!(
            constraints.iter().any(
                |(name, validated, sql)| name == "documents_object_key_check"
                    && *validated
                    && sql == &legacy
            ),
            "known legacy object-key constraint must be present and validated"
        );
        sqlx::query("ALTER TABLE documents DROP CONSTRAINT documents_object_key_check")
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE object_storage_rollout SET tenant_keys_activated=true,activated_at=now(),parser_rollback_floor='tenant-object-keys-v1' WHERE singleton")
            .execute(&mut *tx).await?;
    } else {
        anyhow::ensure!(
            !constraints
                .iter()
                .any(|(name, _, _)| name == "documents_object_key_check"),
            "activated layout must not have the restrictive legacy constraint"
        );
    }
    tx.commit().await?;
    Ok(())
}

async fn project_layout(pool: &PgPool, project: ProjectId) -> anyhow::Result<(Uuid, bool)> {
    Ok(sqlx::query_as("SELECT p.workspace_id,r.tenant_keys_activated FROM projects p CROSS JOIN object_storage_rollout r WHERE p.id=$1 AND r.singleton")
        .bind(project.as_uuid()).fetch_one(pool).await?)
}

/// Factories remain legacy-readable during the N/N-1 overlap.
pub async fn project_document_key(pool: &PgPool, project: ProjectId) -> anyhow::Result<ObjectKey> {
    let (workspace, activated) = project_layout(pool, project).await?;
    let object = Uuid::new_v4();
    Ok(if activated {
        ObjectKey::tenant_document(workspace, project.as_uuid(), object)
    } else {
        ObjectKey::document(object)
    })
}

pub async fn workflow_file_key(
    pool: &PgPool,
    project: ProjectId,
    run: Uuid,
    attempt: Uuid,
) -> anyhow::Result<ObjectKey> {
    let (workspace, activated) = project_layout(pool, project).await?;
    Ok(if activated {
        ObjectKey::tenant_workflow_file(workspace, project.as_uuid(), run, attempt)
    } else {
        ObjectKey::workflow_file(project.as_uuid(), run, attempt)
    })
}

/// Commit a pending intent before external I/O, then hold its row lock through
/// upload and metadata/reference commit. Dropping/rolling back this transaction
/// leaves the committed intent, including when commit success is ambiguous.
pub async fn register_upload_intent(
    pool: &PgPool,
    owner: ObjectOwner,
    key: &ObjectKey,
) -> anyhow::Result<()> {
    let (user, workspace, project) = match owner {
        ObjectOwner::Project(project) => {
            let (workspace, activated) = project_layout(pool, project).await?;
            let tenant = key.as_str().starts_with(&format!(
                "workspaces/{workspace}/projects/{}/",
                project.as_uuid()
            ));
            let legacy = key.as_str().starts_with("documents/")
                || key
                    .as_str()
                    .starts_with(&format!("projects/{}/workflow-runs/", project.as_uuid()));
            anyhow::ensure!(
                legacy || (activated && tenant),
                "upload key does not belong to project or tenant layout is not activated"
            );
            (None, Some(workspace), Some(project.as_uuid()))
        }
        ObjectOwner::Account(user) => {
            anyhow::ensure!(
                key.as_str()
                    .starts_with(&format!("account-exports/{}/", user.as_uuid())),
                "upload key does not belong to account"
            );
            (Some(user.as_uuid()), None, None)
        }
    };
    // A fresh immutable key per attempt is mandatory. Reusing a committed key
    // could overwrite live bytes before a failed metadata commit.
    sqlx::query(
        "INSERT INTO object_lifecycle(storage_key,user_id,workspace_id,project_id,cleanup_after)
                 VALUES($1,$2,$3,$4,now()+interval '24 hours')",
    )
    .bind(key.as_str())
    .bind(user)
    .bind(workspace)
    .bind(project)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn lock_object_for_upload(
    tx: &mut Transaction<'_, Postgres>,
    key: &ObjectKey,
) -> anyhow::Result<()> {
    let deleted: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT retired_at FROM object_lifecycle WHERE storage_key=$1 FOR UPDATE",
    )
    .bind(key.as_str())
    .fetch_one(&mut **tx)
    .await?;
    anyhow::ensure!(deleted.is_none(), "upload intent was already retired");
    Ok(())
}

pub async fn begin_object_upload(
    pool: &PgPool,
    owner: ObjectOwner,
    key: &ObjectKey,
) -> anyhow::Result<Transaction<'static, Postgres>> {
    register_upload_intent(pool, owner, key).await?;
    let mut tx = pool.begin().await?;
    lock_object_for_upload(&mut tx, key).await?;
    Ok(tx)
}

/// Retain a source or completed artifact under the caller's authorized owner.
/// Call in the same transaction that publishes the export/download reference.
pub async fn retain_object_reference(
    tx: &mut Transaction<'_, Postgres>,
    owner: ObjectOwner,
    key: &ObjectKey,
    reference_id: Uuid,
    expires_at: DateTime<Utc>,
) -> anyhow::Result<()> {
    let (user, project) = match owner {
        ObjectOwner::Project(id) => (None, Some(id.as_uuid())),
        ObjectOwner::Account(id) => (Some(id.as_uuid()), None),
    };
    let valid: bool = sqlx::query_scalar(
        "SELECT retired_at IS NULL AND deleted_at IS NULL AND user_id IS NOT DISTINCT FROM $2 AND project_id IS NOT DISTINCT FROM $3
         FROM object_lifecycle WHERE storage_key=$1 FOR UPDATE")
        .bind(key.as_str()).bind(user).bind(project).fetch_one(&mut **tx).await?;
    anyhow::ensure!(
        valid,
        "retained reference does not belong to owner or has been retired"
    );
    anyhow::ensure!(
        expires_at > Utc::now(),
        "retained reference must expire in the future"
    );
    sqlx::query("INSERT INTO object_retained_references(reference_id,storage_key,expires_at)
                 VALUES($1,$2,$3) ON CONFLICT(reference_id,storage_key) DO UPDATE
                 SET expires_at=GREATEST(object_retained_references.expires_at,EXCLUDED.expires_at)")
        .bind(reference_id).bind(key.as_str()).bind(expires_at).execute(&mut **tx).await?;
    sqlx::query("UPDATE object_lifecycle SET cleanup_after=GREATEST(cleanup_after,$2+interval '24 hours') WHERE storage_key=$1")
        .bind(key.as_str()).bind(expires_at).execute(&mut **tx).await?;
    Ok(())
}

pub async fn release_object_reference(
    tx: &mut Transaction<'_, Postgres>,
    reference_id: Uuid,
) -> anyhow::Result<()> {
    // Match cleanup's registry-before-reference lock ordering.
    sqlx::query(
        "SELECT storage_key FROM object_lifecycle WHERE storage_key IN
                 (SELECT storage_key FROM object_retained_references WHERE reference_id=$1)
                 ORDER BY storage_key FOR UPDATE",
    )
    .bind(reference_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query("UPDATE object_lifecycle SET cleanup_after=now()+interval '24 hours'
                 WHERE storage_key IN (SELECT storage_key FROM object_retained_references WHERE reference_id=$1)
                 AND deleted_at IS NULL")
        .bind(reference_id).execute(&mut **tx).await?;
    sqlx::query("DELETE FROM object_retained_references WHERE reference_id=$1")
        .bind(reference_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Retire under a database lock and commit BEFORE external deletion. A crash
/// after S3 deletion but before its acknowledgement cannot reopen attachment:
/// the durable retirement already rejects all new references. Retry NotFound
/// as success, and postpone each storage error without starving other keys.
pub async fn cleanup_objects(
    pool: &PgPool,
    store: &DocumentStore,
    limit: i64,
) -> anyhow::Result<u64> {
    anyhow::ensure!((1..=1000).contains(&limit), "cleanup batch must be 1..1000");
    let mut count = 0;
    for _ in 0..limit {
        let mut tx = pool.begin().await?;
        let candidate: Option<(String, Option<DateTime<Utc>>)> = sqlx::query_as(
            "SELECT storage_key,retired_at FROM object_lifecycle WHERE deleted_at IS NULL
             AND cleanup_after <= now() ORDER BY cleanup_after,storage_key FOR UPDATE SKIP LOCKED LIMIT 1")
            .fetch_optional(&mut *tx).await?;
        let Some((key, retired)) = candidate else {
            break;
        };
        if retired.is_none() {
            let live: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM documents WHERE object_key=$1)
                 OR EXISTS(SELECT 1 FROM workflow_files WHERE storage_key=$1)",
            )
            .bind(&key)
            .fetch_one(&mut *tx)
            .await?;
            if live {
                sqlx::query("UPDATE object_lifecycle SET cleanup_after=NULL WHERE storage_key=$1")
                    .bind(&key)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                continue;
            }
            let expiry: Option<DateTime<Utc>> = sqlx::query_scalar(
                "SELECT max(expires_at) FROM object_retained_references WHERE storage_key=$1 AND expires_at>now()")
                .bind(&key).fetch_one(&mut *tx).await?;
            if let Some(expiry) = expiry {
                sqlx::query("UPDATE object_lifecycle SET cleanup_after=$2+interval '24 hours' WHERE storage_key=$1")
                    .bind(&key).bind(expiry).execute(&mut *tx).await?;
                tx.commit().await?;
                continue;
            }
            sqlx::query("UPDATE object_lifecycle SET retired_at=now() WHERE storage_key=$1")
                .bind(&key)
                .execute(&mut *tx)
                .await?;
        }
        // This commit is the irreversible deletion-intent boundary. Never
        // combine it with the transaction acknowledging external deletion.
        tx.commit().await?;
        let mut tx = pool.begin().await?;
        let unfinished: bool = sqlx::query_scalar(
            "SELECT deleted_at IS NULL FROM object_lifecycle WHERE storage_key=$1 FOR UPDATE",
        )
        .bind(&key)
        .fetch_one(&mut *tx)
        .await?;
        if !unfinished {
            tx.rollback().await?;
            continue;
        }
        let deletion = async {
            let parsed = ObjectKey::parse(&key)?;
            store.delete_object(&parsed).await
        }
        .await;
        match deletion {
            Ok(()) => {}
            Err(error) if error.is_not_found() => {}
            Err(error) => {
                tracing::warn!(%key, %error, "object cleanup deferred after storage failure");
                sqlx::query("UPDATE object_lifecycle SET cleanup_after=now()+interval '5 minutes',cleanup_attempts=cleanup_attempts+1,last_error=$2 WHERE storage_key=$1")
                    .bind(&key).bind(error.to_string().chars().take(1024).collect::<String>()).execute(&mut *tx).await?;
                tx.commit().await?;
                continue;
            }
        }
        sqlx::query("UPDATE object_lifecycle SET deleted_at=now(),cleanup_after=NULL,last_error=NULL WHERE storage_key=$1")
            .bind(&key).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM object_retained_references WHERE storage_key=$1")
            .bind(&key)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        count += 1;
    }
    Ok(count)
}

pub async fn schedule_object_cleanup(pool: &PgPool) -> anyhow::Result<()> {
    crate::account_export::expire_exports(pool).await?;
    let due: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM object_lifecycle WHERE deleted_at IS NULL AND cleanup_after <= now())")
        .fetch_one(pool).await?;
    if due {
        crate::enqueue_job_pool(
            pool,
            &crate::scoped_job(
                Uuid::new_v4(),
                JobScope::Platform,
                JobKind::CleanupObjects,
                serde_json::json!({}),
                format!("cleanup_objects:{}", Utc::now().format("%Y%m%d%H%M")),
            ),
        )
        .await?;
    }
    Ok(())
}

/// Adopt pre-intent orphans only after all old uploaders are stopped. Complete
/// listing and an age floor avoid deleting in-flight legacy uploads. Unknown
/// key shapes remain untouched. Cleanup still rechecks all references under lock.
pub async fn reconcile_legacy_orphans(
    pool: &PgPool,
    store: &DocumentStore,
    maximum: usize,
    attestation: &str,
) -> anyhow::Result<u64> {
    anyhow::ensure!(
        attestation == "all-legacy-uploaders-drained",
        "legacy uploader drain attestation required"
    );
    anyhow::ensure!(
        (1..=100_000).contains(&maximum),
        "inventory bound must be 1..100000"
    );
    let inventory = store.inventory(maximum).await?;
    let stale_before = (Utc::now() - chrono::Duration::hours(24)).timestamp();
    let mut tx = pool.begin().await?;
    let mut registered = 0;
    for (key, modified) in inventory {
        if modified > stale_before {
            continue;
        }
        registered += sqlx::query(
            "INSERT INTO object_lifecycle(storage_key,cleanup_after)
             SELECT $1,now()+interval '24 hours'
             WHERE NOT EXISTS(SELECT 1 FROM documents WHERE object_key=$1)
               AND NOT EXISTS(SELECT 1 FROM workflow_files WHERE storage_key=$1)
             ON CONFLICT DO NOTHING",
        )
        .bind(key.as_str())
        .execute(&mut *tx)
        .await?
        .rows_affected();
    }
    tx.commit().await?;
    Ok(registered)
}
