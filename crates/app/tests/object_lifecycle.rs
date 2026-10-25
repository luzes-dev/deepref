#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
use bytes::Bytes;
use chrono::{Duration, Utc};
use deepref_app::object_lifecycle::*;
use deepref_app::{
    NewWorkflowFile, WorkflowFileCutover, create_workflow_file, cutover_workflow_file_storage,
    get_workflow_file,
};
use deepref_documents::{DocumentStore, ObjectKey};
use deepref_domain::{ProjectId, UserId};
use sqlx::PgPool;
use uuid::Uuid;

async fn database() -> Option<PgPool> {
    deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
}
async fn fixture(pool: &PgPool) -> (Uuid, Uuid) {
    let project: Uuid = sqlx::query_scalar(
        "INSERT INTO projects(id,name) VALUES(gen_random_uuid(),'object lifecycle') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let run: Uuid = sqlx::query_scalar(
        "INSERT INTO durable_runs(id,project_id,kind,actor_kind,actor_id,idempotency_key,status)
        VALUES(gen_random_uuid(),$1,'workflow','user','fixture',$2,'queued') RETURNING id",
    )
    .bind(project)
    .bind(Uuid::new_v4().to_string())
    .fetch_one(pool)
    .await
    .unwrap();
    (project, run)
}
async fn file(pool: &PgPool, store: &DocumentStore, project: Uuid, run: Uuid) -> (Uuid, ObjectKey) {
    let id = create_workflow_file(
        pool,
        store,
        NewWorkflowFile {
            project_id: project,
            run_id: run,
            node_id: "export",
            name: "a.csv",
            content_type: "text/csv",
            content: b"a,b",
        },
    )
    .await
    .unwrap();
    let key: String = sqlx::query_scalar("SELECT storage_key FROM workflow_files WHERE id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap();
    (id, ObjectKey::parse(&key).unwrap())
}
async fn due(pool: &PgPool) {
    sqlx::query("UPDATE object_lifecycle SET cleanup_after=now()-interval '1 second' WHERE deleted_at IS NULL")
        .execute(pool).await.unwrap();
}
#[tokio::test]
async fn deleted_files_keep_transactional_intent_and_retry_missing_objects() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let (project, run) = fixture(&pool).await;
    let (id, key) = file(&pool, &store, project, run).await;
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("DELETE FROM workflow_files WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    due(&pool).await;
    assert_eq!(cleanup_objects(&pool, &store, 10).await.unwrap(), 0);
    assert!(store.exists(key.as_str()).await.unwrap());
    sqlx::query("DELETE FROM workflow_files WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    due(&pool).await;
    // Crash after S3 delete but before DB commit: NotFound must complete replay.
    store.delete_object(&key).await.unwrap();
    assert_eq!(cleanup_objects(&pool, &store, 10).await.unwrap(), 1);
    assert_eq!(cleanup_objects(&pool, &store, 10).await.unwrap(), 0);
    let retired: bool = sqlx::query_scalar(
        "SELECT deleted_at IS NOT NULL FROM object_lifecycle WHERE storage_key=$1",
    )
    .bind(key.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(retired);
}
#[tokio::test]
async fn retained_export_source_survives_policy_approved_project_deletion_until_expiry() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let (project, run) = fixture(&pool).await;
    let (_, key) = file(&pool, &store, project, run).await;
    let reference = Uuid::new_v4();
    let mut tx = pool.begin().await.unwrap();
    retain_object_reference(
        &mut tx,
        ObjectOwner::Project(ProjectId::new(project)),
        &key,
        reference,
        Utc::now() + Duration::days(2),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    deepref_app::delete_project_in_transaction(&mut tx, project)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    due(&pool).await;
    assert_eq!(cleanup_objects(&pool, &store, 10).await.unwrap(), 0);
    assert!(store.exists(key.as_str()).await.unwrap());
    sqlx::query("UPDATE object_retained_references SET expires_at=now()-interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    due(&pool).await;
    assert_eq!(cleanup_objects(&pool, &store, 10).await.unwrap(), 1);
}
#[tokio::test]
async fn account_export_pending_upload_is_durable_and_fenced_against_reuse() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let user = UserId::new(Uuid::new_v4());
    let key = ObjectKey::account_export(user.as_uuid(), Uuid::new_v4());
    let mut tx = begin_object_upload(&pool, ObjectOwner::Account(user), &key)
        .await
        .unwrap();
    let stored = store
        .for_upload(key.clone())
        .put_stream(futures::stream::iter([Ok::<_, String>(
            Bytes::from_static(b"zip"),
        )]))
        .await
        .unwrap();
    assert_eq!(stored.opaque_id, key.as_str());
    retain_object_reference(
        &mut tx,
        ObjectOwner::Account(user),
        &key,
        Uuid::new_v4(),
        Utc::now() + Duration::hours(1),
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    due(&pool).await;
    assert_eq!(cleanup_objects(&pool, &store, 10).await.unwrap(), 1);
    assert!(
        begin_object_upload(&pool, ObjectOwner::Account(user), &key)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn pending_upload_lock_prevents_cleanup_and_references_reject_foreign_owner() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let user = UserId::new(Uuid::new_v4());
    let key = ObjectKey::account_export(user.as_uuid(), Uuid::new_v4());
    let mut tx = begin_object_upload(&pool, ObjectOwner::Account(user), &key)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE object_lifecycle SET cleanup_after=now()-interval '1 second' WHERE storage_key=$1",
    )
    .bind(key.as_str())
    .execute(&mut *tx)
    .await
    .unwrap();
    assert_eq!(cleanup_objects(&pool, &store, 10).await.unwrap(), 0);
    assert!(
        retain_object_reference(
            &mut tx,
            ObjectOwner::Account(UserId::new(Uuid::new_v4())),
            &key,
            Uuid::new_v4(),
            Utc::now() + Duration::hours(1)
        )
        .await
        .is_err()
    );
    tx.rollback().await.unwrap();
    due(&pool).await;
    let mut locked = pool.begin().await.unwrap();
    sqlx::query("SELECT storage_key FROM object_lifecycle WHERE storage_key=$1 FOR UPDATE")
        .bind(key.as_str())
        .execute(&mut *locked)
        .await
        .unwrap();
    assert_eq!(cleanup_objects(&pool, &store, 10).await.unwrap(), 0);
    locked.rollback().await.unwrap();
    assert_eq!(cleanup_objects(&pool, &store, 10).await.unwrap(), 1);
}
#[tokio::test]
async fn live_shared_reference_prevents_cleanup_and_tombstone_prevents_late_attachment() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let (project, run) = fixture(&pool).await;
    let (id, key) = file(&pool, &store, project, run).await;
    sqlx::query("INSERT INTO workflow_files(project_id,run_id,node_id,name,content_type,content,storage_key,content_sha256,byte_size)
        SELECT project_id,run_id,node_id,'copy',content_type,content,storage_key,content_sha256,byte_size FROM workflow_files WHERE id=$1")
        .bind(id).execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM workflow_files WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    due(&pool).await;
    assert_eq!(cleanup_objects(&pool, &store, 10).await.unwrap(), 0);
    sqlx::query("DELETE FROM workflow_files WHERE project_id=$1")
        .bind(project)
        .execute(&pool)
        .await
        .unwrap();
    due(&pool).await;
    assert_eq!(cleanup_objects(&pool, &store, 10).await.unwrap(), 1);
    assert!(sqlx::query("INSERT INTO workflow_files(project_id,run_id,node_id,name,content_type,content,storage_key,content_sha256,byte_size)
        VALUES($1,$2,'x','x','text/plain','x',$3,repeat('a',64),1)")
        .bind(project).bind(run).bind(key.as_str()).execute(&pool).await.is_err());
}
#[tokio::test]
async fn staged_cutover_requires_drains_complete_backfill_and_verified_objects() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let (project, run) = fixture(&pool).await;
    let (id, key) = file(&pool, &store, project, run).await;
    assert!(
        cutover_workflow_file_storage(&pool, &store, WorkflowFileCutover::ObjectOnly, "yes")
            .await
            .is_err()
    );
    store
        .put_object(&key, Bytes::from_static(b"bad"), 1024)
        .await
        .unwrap();
    assert!(
        cutover_workflow_file_storage(
            &pool,
            &store,
            WorkflowFileCutover::ObjectOnly,
            WorkflowFileCutover::ObjectOnly.drain_attestation()
        )
        .await
        .is_err()
    );
    store
        .put_object(&key, Bytes::from_static(b"a,b"), 1024)
        .await
        .unwrap();
    assert_eq!(
        cutover_workflow_file_storage(
            &pool,
            &store,
            WorkflowFileCutover::ObjectOnly,
            WorkflowFileCutover::ObjectOnly.drain_attestation()
        )
        .await
        .unwrap(),
        1
    );
    // An expanded N-1 dual writer still has all reference columns, but
    // must not repopulate compatibility bodies after explicit cutover.
    assert!(
        sqlx::query("UPDATE workflow_files SET content='old-writer' WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .is_err()
    );
    let (object_only, _) = file(&pool, &store, project, run).await;
    let content: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT content FROM workflow_files WHERE id=$1")
            .bind(object_only)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(content.is_none());
    assert!(sqlx::query("INSERT INTO workflow_files(project_id,run_id,node_id,name,content_type,content) VALUES($1,$2,'old','old','text/plain','old')")
        .bind(project).bind(run).execute(&pool).await.is_err());
    assert_eq!(
        cutover_workflow_file_storage(
            &pool,
            &store,
            WorkflowFileCutover::Contract,
            WorkflowFileCutover::Contract.drain_attestation()
        )
        .await
        .unwrap(),
        2
    );
    let (contracted, _) = file(&pool, &store, project, run).await;
    for file_id in [id, object_only, contracted] {
        assert_eq!(
            get_workflow_file(&pool, &store, ProjectId::new(project), file_id)
                .await
                .unwrap()
                .content,
            b"a,b"
        );
    }
    store.delete_object(&key).await.unwrap();
    assert!(
        get_workflow_file(&pool, &store, ProjectId::new(project), id)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn n_minus_one_cannot_claim_cleanup_or_starve_ordinary_work() {
    let Some(pool) = database().await else { return };
    sqlx::query("INSERT INTO jobs(id,scope,kind,payload,dedupe_key,workload_class) SELECT gen_random_uuid(),'platform','cleanup_objects','{}',gen_random_uuid()::text,'documents' FROM generate_series(1,20)")
        .execute(&pool).await.unwrap();
    let ordinary: Uuid=sqlx::query_scalar("INSERT INTO jobs(id,scope,kind,payload,dedupe_key,workload_class) VALUES(gen_random_uuid(),'platform','recompute_metrics','{}','ordinary','projection') RETURNING id")
        .fetch_one(&pool).await.unwrap();
    let claimed: Vec<Uuid>=sqlx::query_scalar("UPDATE jobs SET state='running',lease_owner='old',leased_until=now()+interval '1 minute'
        WHERE id IN(SELECT id FROM jobs WHERE state='queued' ORDER BY available_at LIMIT 1) RETURNING id")
        .fetch_all(&pool).await.unwrap();
    assert_eq!(claimed, vec![ordinary]);
    let protected: i64=sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='cleanup_objects' AND state='capability_wait' AND attempts=0")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(protected, 20);
    let direct: Vec<Uuid>=sqlx::query_scalar("UPDATE jobs SET state='running',lease_owner='old' WHERE kind='cleanup_objects' RETURNING id")
        .fetch_all(&pool).await.unwrap();
    assert!(direct.is_empty());
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('deepref.job_kinds','cleanup_objects',true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET state='running',lease_owner='capable',leased_until=now()-interval '1 second' WHERE id IN(SELECT id FROM jobs WHERE kind='cleanup_objects' LIMIT 1)")
        .execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let recovered: Vec<String>=sqlx::query_scalar("UPDATE jobs SET state='queued',lease_owner=NULL WHERE kind='cleanup_objects' AND state='running' AND leased_until<now() RETURNING kind")
        .fetch_all(&pool).await.unwrap();
    assert!(
        recovered.is_empty(),
        "N-1 recovery must never decode the unknown kind"
    );
}

#[tokio::test]
async fn unbackfilled_rows_block_cutover_and_ordinary_migration_keeps_not_null() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let (project, run) = fixture(&pool).await;
    sqlx::query("INSERT INTO workflow_files(project_id,run_id,node_id,name,content_type,content) VALUES($1,$2,'old','old','text/plain','old')")
        .bind(project).bind(run).execute(&pool).await.unwrap();
    assert!(
        cutover_workflow_file_storage(
            &pool,
            &store,
            WorkflowFileCutover::ObjectOnly,
            WorkflowFileCutover::ObjectOnly.drain_attestation()
        )
        .await
        .is_err()
    );
    let required: bool=sqlx::query_scalar("SELECT attnotnull FROM pg_attribute WHERE attrelid='workflow_files'::regclass AND attname='content'")
        .fetch_one(&pool).await.unwrap();
    assert!(required);
    assert_eq!(
        deepref_app::backfill_workflow_files(&pool, &store, ProjectId::new(project), 10)
            .await
            .unwrap()
            .migrated,
        1
    );
    assert_eq!(
        cutover_workflow_file_storage(
            &pool,
            &store,
            WorkflowFileCutover::ObjectOnly,
            WorkflowFileCutover::ObjectOnly.drain_attestation()
        )
        .await
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn replacement_and_workspace_cascade_preserve_cleanup_intent() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let (project, run) = fixture(&pool).await;
    let (id, old) = file(&pool, &store, project, run).await;
    let (other, current) = file(&pool, &store, project, run).await;
    // Shared current key survives deletion of its first referencing row.
    sqlx::query("UPDATE workflow_files SET storage_key=$2 WHERE id=$1")
        .bind(id)
        .bind(current.as_str())
        .execute(&pool)
        .await
        .unwrap();
    due(&pool).await;
    assert_eq!(cleanup_objects(&pool, &store, 10).await.unwrap(), 1);
    assert!(!store.exists(old.as_str()).await.unwrap());
    sqlx::query("DELETE FROM workflow_files WHERE id=$1")
        .bind(other)
        .execute(&pool)
        .await
        .unwrap();
    due(&pool).await;
    assert_eq!(cleanup_objects(&pool, &store, 10).await.unwrap(), 0);
    // Privacy performs ordered project deletion first (required by scientific
    // RESTRICT children); the registry survives the subsequent tenant removal.
    let workspace: Uuid = sqlx::query_scalar("SELECT workspace_id FROM projects WHERE id=$1")
        .bind(project)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    deepref_app::delete_project_in_transaction(&mut tx, project)
        .await
        .unwrap();
    sqlx::query("DELETE FROM workspaces WHERE id=$1")
        .bind(workspace)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    due(&pool).await;
    assert_eq!(cleanup_objects(&pool, &store, 10).await.unwrap(), 1);
}

#[tokio::test]
async fn tenant_key_and_legacy_reference_access_remain_isolated() {
    let Some(pool) = database().await else { return };
    activate_tenant_object_keys(&pool, TENANT_KEY_DRAIN_ATTESTATION)
        .await
        .unwrap();
    let store = DocumentStore::memory();
    let (project, run) = fixture(&pool).await;
    let (id, key) = file(&pool, &store, project, run).await;
    let (foreign, foreign_run) = fixture(&pool).await;
    assert!(
        begin_object_upload(&pool, ObjectOwner::Project(ProjectId::new(foreign)), &key)
            .await
            .is_err()
    );
    assert!(sqlx::query("INSERT INTO workflow_files(project_id,run_id,node_id,name,content_type,content,storage_key,content_sha256,byte_size)
        VALUES($1,$2,'x','x','text/plain','a,b',$3,repeat('a',64),3)")
        .bind(foreign).bind(foreign_run).bind(key.as_str()).execute(&pool).await.is_err());
    assert!(
        get_workflow_file(&pool, &store, ProjectId::new(foreign), id)
            .await
            .is_err()
    );
    let legacy = ObjectKey::workflow_file(project, run, id);
    store
        .put_object(&legacy, Bytes::from_static(b"a,b"), 1024)
        .await
        .unwrap();
    sqlx::query("UPDATE workflow_files SET storage_key=$2 WHERE id=$1")
        .bind(id)
        .bind(legacy.as_str())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        get_workflow_file(&pool, &store, ProjectId::new(project), id)
            .await
            .unwrap()
            .content,
        b"a,b"
    );
}

#[tokio::test]
async fn released_account_export_retention_is_durable_and_does_not_release_live_sources() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let (project, run) = fixture(&pool).await;
    let (_, source) = file(&pool, &store, project, run).await;
    let user = UserId::new(Uuid::new_v4());
    let key = ObjectKey::account_export(user.as_uuid(), Uuid::new_v4());
    let reference = Uuid::new_v4();
    let mut tx = begin_object_upload(&pool, ObjectOwner::Account(user), &key)
        .await
        .unwrap();
    store
        .put_object(&key, Bytes::from_static(b"ZIP"), 1024)
        .await
        .unwrap();
    retain_object_reference(
        &mut tx,
        ObjectOwner::Account(user),
        &key,
        reference,
        Utc::now() + Duration::hours(1),
    )
    .await
    .unwrap();
    retain_object_reference(
        &mut tx,
        ObjectOwner::Project(ProjectId::new(project)),
        &source,
        reference,
        Utc::now() + Duration::hours(1),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    due(&pool).await;
    assert_eq!(cleanup_objects(&pool, &store, 10).await.unwrap(), 0);
    let mut tx = pool.begin().await.unwrap();
    release_object_reference(&mut tx, reference).await.unwrap();
    tx.commit().await.unwrap();
    due(&pool).await;
    assert_eq!(cleanup_objects(&pool, &store, 10).await.unwrap(), 1);
    assert!(store.exists(source.as_str()).await.unwrap());
    assert!(!store.exists(key.as_str()).await.unwrap());
}

#[cfg(unix)]
#[tokio::test]
async fn storage_delete_failure_keeps_retry_intent_without_starving_other_objects() {
    use std::os::unix::fs::PermissionsExt;
    let Some(pool) = database().await else { return };
    let root = std::env::temp_dir().join(format!("deepref-delete-retry-{}", Uuid::new_v4()));
    let store = DocumentStore::new(deepref_documents::StoreConfig::local(&root).unwrap(), 1024);
    let (project, run) = fixture(&pool).await;
    let (id, key) = file(&pool, &store, project, run).await;
    sqlx::query("DELETE FROM workflow_files WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    due(&pool).await;
    let user = UserId::new(Uuid::new_v4());
    let healthy = ObjectKey::account_export(user.as_uuid(), Uuid::new_v4());
    let pending = begin_object_upload(&pool, ObjectOwner::Account(user), &healthy)
        .await
        .unwrap();
    store
        .put_object(&healthy, Bytes::from_static(b"ZIP"), 1024)
        .await
        .unwrap();
    pending.rollback().await.unwrap();
    due(&pool).await;
    let parent = root.join(key.as_str()).parent().unwrap().to_owned();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o500)).unwrap();
    assert_eq!(cleanup_objects(&pool, &store, 10).await.unwrap(), 1);
    assert!(!store.exists(healthy.as_str()).await.unwrap());
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    let attempts: i64 = sqlx::query_scalar(
        "SELECT cleanup_attempts FROM object_lifecycle WHERE storage_key=$1 AND deleted_at IS NULL",
    )
    .bind(key.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attempts, 1);
    let retired: bool=sqlx::query_scalar("SELECT retired_at IS NOT NULL AND deleted_at IS NULL FROM object_lifecycle WHERE storage_key=$1")
        .bind(key.as_str()).fetch_one(&pool).await.unwrap();
    assert!(
        retired,
        "storage failure must preserve the committed retirement fence"
    );
    assert!(sqlx::query("INSERT INTO workflow_files(project_id,run_id,node_id,name,content_type,content,storage_key,content_sha256,byte_size)
        VALUES($1,$2,'late','late','text/plain','a,b',$3,$4,3)")
        .bind(project).bind(run).bind(key.as_str()).bind(deepref_documents::content_sha256(b"a,b")).execute(&pool).await.is_err());
    due(&pool).await;
    assert_eq!(cleanup_objects(&pool, &store, 10).await.unwrap(), 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn legacy_orphan_adoption_requires_complete_inventory_old_age_and_uploader_drain() {
    let Some(pool) = database().await else { return };
    let root = std::env::temp_dir().join(format!("deepref-orphans-{}", Uuid::new_v4()));
    let store = DocumentStore::new(deepref_documents::StoreConfig::local(&root).unwrap(), 1024);
    let old = ObjectKey::document(Uuid::new_v4());
    let recent = ObjectKey::document(Uuid::new_v4());
    for key in [&old, &recent] {
        store
            .put_object(key, Bytes::from_static(b"pdf"), 1024)
            .await
            .unwrap();
    }
    std::fs::File::open(root.join(old.as_str()))
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(48 * 3600))
        .unwrap();
    assert!(
        reconcile_legacy_orphans(&pool, &store, 10, "yes")
            .await
            .is_err()
    );
    assert!(
        reconcile_legacy_orphans(&pool, &store, 1, "all-legacy-uploaders-drained")
            .await
            .is_err()
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM object_lifecycle")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        reconcile_legacy_orphans(&pool, &store, 10, "all-legacy-uploaders-drained")
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        reconcile_legacy_orphans(&pool, &store, 10, "all-legacy-uploaders-drained")
            .await
            .unwrap(),
        0
    );
    due(&pool).await;
    assert_eq!(cleanup_objects(&pool, &store, 10).await.unwrap(), 1);
    assert!(store.exists(recent.as_str()).await.unwrap());
    std::fs::remove_dir_all(root).unwrap();
}

async fn rollout_state(pool: &PgPool) -> (bool, Option<String>, bool) {
    sqlx::query_as("SELECT tenant_keys_activated,parser_rollback_floor,
        EXISTS(SELECT 1 FROM pg_constraint WHERE conrelid='documents'::regclass AND conname='documents_object_key_check')
        FROM object_storage_rollout WHERE singleton").fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn tenant_layout_keeps_legacy_overlap_until_exact_guarded_activation() {
    let Some(pool) = database().await else { return };
    let store = DocumentStore::memory();
    let (project, run) = fixture(&pool).await;
    let before = rollout_state(&pool).await;
    assert_eq!(before, (false, None, true));
    let legacy_doc = project_document_key(&pool, ProjectId::new(project))
        .await
        .unwrap();
    assert!(legacy_doc.as_str().starts_with("documents/"));
    let (old_id, old_key) = file(&pool, &store, project, run).await;
    assert!(
        old_key
            .as_str()
            .starts_with(&format!("projects/{project}/workflow-runs/{run}/files/"))
    );
    let workspace: Uuid = sqlx::query_scalar("SELECT workspace_id FROM projects WHERE id=$1")
        .bind(project)
        .fetch_one(&pool)
        .await
        .unwrap();
    let unactivated = ObjectKey::tenant_document(workspace, project, Uuid::new_v4());
    assert!(
        begin_object_upload(
            &pool,
            ObjectOwner::Project(ProjectId::new(project)),
            &unactivated
        )
        .await
        .is_err()
    );
    let intents: i64 =
        sqlx::query_scalar("SELECT count(*) FROM object_lifecycle WHERE storage_key=$1")
            .bind(unactivated.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(intents, 0);
    assert!(activate_tenant_object_keys(&pool, "yes").await.is_err());
    assert_eq!(rollout_state(&pool).await, before);
    activate_tenant_object_keys(&pool, TENANT_KEY_DRAIN_ATTESTATION)
        .await
        .unwrap();
    assert_eq!(
        rollout_state(&pool).await,
        (true, Some("tenant-object-keys-v1".to_owned()), false)
    );
    activate_tenant_object_keys(&pool, TENANT_KEY_DRAIN_ATTESTATION)
        .await
        .unwrap();
    let key = project_document_key(&pool, ProjectId::new(project))
        .await
        .unwrap();
    assert!(key.as_str().starts_with(&format!(
        "workspaces/{workspace}/projects/{project}/documents/"
    )));
    let tx = begin_object_upload(&pool, ObjectOwner::Project(ProjectId::new(project)), &key)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let (new_id, new_key) = file(&pool, &store, project, run).await;
    assert!(new_key.as_str().starts_with(&format!(
        "workspaces/{workspace}/projects/{project}/workflow-runs/{run}/files/"
    )));
    for id in [old_id, new_id] {
        assert_eq!(
            get_workflow_file(&pool, &store, ProjectId::new(project), id)
                .await
                .unwrap()
                .content,
            b"a,b"
        );
    }
}

#[tokio::test]
async fn tenant_layout_activation_refuses_incomplete_validation_or_unexpected_legacy_check() {
    let Some(pool) = database().await else { return };
    sqlx::query("ALTER TABLE documents DROP CONSTRAINT documents_object_key_tenant_layout_check")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE documents ADD CONSTRAINT documents_object_key_tenant_layout_check CHECK(object_key IS NULL OR object_key <> '') NOT VALID").execute(&pool).await.unwrap();
    assert!(
        activate_tenant_object_keys(&pool, TENANT_KEY_DRAIN_ATTESTATION)
            .await
            .is_err()
    );
    assert_eq!(rollout_state(&pool).await, (false, None, true));
    // Restore the actual migration's widened CHECK and validate it; replacing
    // the old CHECK with unexpected SQL must still fail without dropping it.
    sqlx::query("ALTER TABLE documents DROP CONSTRAINT documents_object_key_tenant_layout_check")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../migrations/0146_tenant_document_keys.sql"))
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        activate_tenant_object_keys(&pool, TENANT_KEY_DRAIN_ATTESTATION)
            .await
            .is_err(),
        "correct widened SQL still requires validation"
    );
    assert_eq!(rollout_state(&pool).await, (false, None, true));
    sqlx::raw_sql(include_str!(
        "../migrations/0147_validate_tenant_document_keys.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("ALTER TABLE documents DROP CONSTRAINT documents_object_key_check")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE documents ADD CONSTRAINT documents_object_key_check CHECK(object_key IS NULL OR object_key <> '')").execute(&pool).await.unwrap();
    assert!(
        activate_tenant_object_keys(&pool, TENANT_KEY_DRAIN_ATTESTATION)
            .await
            .is_err()
    );
    assert_eq!(rollout_state(&pool).await, (false, None, true));
}

#[tokio::test]
async fn tenant_layout_activation_does_not_partially_relax_while_a_producer_holds_the_table() {
    let Some(pool) = database().await else { return };
    let mut producer = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE documents IN ROW EXCLUSIVE MODE")
        .execute(&mut *producer)
        .await
        .unwrap();
    assert!(
        activate_tenant_object_keys(&pool, TENANT_KEY_DRAIN_ATTESTATION)
            .await
            .is_err()
    );
    assert_eq!(rollout_state(&pool).await, (false, None, true));
    producer.rollback().await.unwrap();
    activate_tenant_object_keys(&pool, TENANT_KEY_DRAIN_ATTESTATION)
        .await
        .unwrap();
    assert_eq!(
        rollout_state(&pool).await,
        (true, Some("tenant-object-keys-v1".to_owned()), false)
    );
}

#[tokio::test]
async fn tenant_layout_activation_rolls_back_the_check_drop_if_persisting_the_floor_fails() {
    let Some(pool) = database().await else { return };
    sqlx::raw_sql("CREATE FUNCTION reject_fixture_activation() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected rollout persistence failure'; END; $$;
        CREATE TRIGGER reject_fixture_activation BEFORE UPDATE ON object_storage_rollout FOR EACH ROW EXECUTE FUNCTION reject_fixture_activation();")
        .execute(&pool).await.unwrap();
    assert!(
        activate_tenant_object_keys(&pool, TENANT_KEY_DRAIN_ATTESTATION)
            .await
            .is_err()
    );
    assert_eq!(
        rollout_state(&pool).await,
        (false, None, true),
        "transaction rollback must restore the dropped legacy CHECK too"
    );
    sqlx::query("DROP TRIGGER reject_fixture_activation ON object_storage_rollout")
        .execute(&pool)
        .await
        .unwrap();
    activate_tenant_object_keys(&pool, TENANT_KEY_DRAIN_ATTESTATION)
        .await
        .unwrap();
    assert_eq!(
        rollout_state(&pool).await,
        (true, Some("tenant-object-keys-v1".to_owned()), false)
    );
}
