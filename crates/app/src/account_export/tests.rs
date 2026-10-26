#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
use super::*;
use deepref_documents::StoreConfig;
use deepref_domain::ProjectId;

async fn setup() -> Option<(PgPool, UserId, Uuid, ProjectId, DocumentStore, zip::Staging)> {
    let pool = crate::test_isolation::isolated_pool_from_env("DATABASE_URL", 1).await?;
    let user = UserId::new(Uuid::new_v4());
    let workspace = Uuid::new_v4();
    let project = ProjectId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO users(id,workos_user_id,primary_email,status) VALUES($1,'export-race','race@example.org','active')").bind(user.as_uuid()).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO workspaces(id,kind,name,slug) VALUES($1,'personal','Race','race')")
        .bind(workspace)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO workspace_memberships(id,workspace_id,user_id,role,status,grant_source) VALUES($1,$2,$3,'owner','active','local')").bind(Uuid::new_v4()).bind(workspace).bind(user.as_uuid()).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO projects(id,name,workspace_id) VALUES($1,'Race',$2)")
        .bind(project.as_uuid())
        .bind(workspace)
        .execute(&pool)
        .await
        .unwrap();
    activate_export_retention(&pool, EXPORT_ATTESTATION_TOKEN)
        .await
        .unwrap();
    let dir = zip::Staging::new().unwrap();
    let store = DocumentStore::new(
        StoreConfig::local(dir.path()).unwrap(),
        MAX_ARCHIVE_BYTES as usize,
    );
    Some((pool, user, workspace, project, store, dir))
}
async fn begin(pool: &PgPool, user: UserId) -> (Attempt, JobLease) {
    let export = request_account_export(pool, user, crate::identity::AuditRequest::default())
        .await
        .unwrap();
    let claimed =
        crate::jobs::claim_jobs(pool, "race", std::time::Duration::from_secs(3600), &[], 1)
            .await
            .unwrap();
    let lease = claimed[0].lease("race");
    (
        start_attempt(pool, export.id, &lease)
            .await
            .unwrap()
            .unwrap(),
        lease,
    )
}

#[tokio::test]
async fn committed_source_retention_and_post_collection_revocation() {
    for race in ["demote", "deactivate", "cancel"] {
        let Some((pool, user, workspace, project, store, _storage)) = setup().await else {
            return;
        };
        let key = ObjectKey::document(Uuid::new_v4());
        let stored = store
            .for_upload(key.clone())
            .put_stream(futures::stream::iter([Ok::<_, std::io::Error>(
                bytes::Bytes::from_static(b"exact retained bytes"),
            )]))
            .await
            .unwrap();
        let document = Uuid::new_v4();
        sqlx::query("INSERT INTO reports(id,title) VALUES($1,'Retained')")
            .bind(document)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
            .bind(project.as_uuid())
            .bind(document)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO documents(id,report_id,project_id,object_key,content_hash,mime_type,byte_size,status) VALUES($1,$1,$2,$3,$4,'application/pdf',$5,'uploaded')").bind(document).bind(project.as_uuid()).bind(key.as_str()).bind(stored.sha256).bind(stored.byte_size as i64).execute(&pool).await.unwrap();
        let (attempt, lease) = begin(&pool, user).await;
        let staging = zip::Staging::new().unwrap();
        let (inventory, until) = prepare_attempt(&pool, &attempt, &lease, staging.path())
            .await
            .unwrap();
        let owner: Uuid =
            sqlx::query_scalar("SELECT project_id FROM object_lifecycle WHERE storage_key=$1")
                .bind(key.as_str())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(owner, project.as_uuid());
        // Metadata disappears before the first object read. Retention must have
        // committed already, or this cleanup retires and deletes the bytes.
        sqlx::query("DELETE FROM documents WHERE id=$1")
            .bind(document)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE object_lifecycle SET cleanup_after=now()-interval '1 second' WHERE storage_key=$1").bind(key.as_str()).execute(&pool).await.unwrap();
        object_lifecycle::cleanup_objects(&pool, &store, 10)
            .await
            .unwrap();
        assert!(store.get(key.as_str()).await.is_ok());
        let summary = zip::package(&store, &inventory, staging.path(), user)
            .await
            .unwrap();
        match race {
            "demote" => {
                sqlx::query("UPDATE workspace_memberships SET role='member' WHERE workspace_id=$1 AND user_id=$2").bind(workspace).bind(user.as_uuid()).execute(&pool).await.unwrap();
            }
            "deactivate" => {
                sqlx::query("UPDATE users SET status='deactivated' WHERE id=$1")
                    .bind(user.as_uuid())
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            "cancel" => {
                cancel_account_export(
                    &pool,
                    user,
                    attempt.export,
                    crate::identity::AuditRequest::default(),
                )
                .await
                .unwrap();
            }
            _ => unreachable!(),
        }
        let published =
            publish_attempt(&pool, &store, &attempt, &lease, &inventory, &summary, until).await;
        if race == "cancel" {
            assert!(published.is_err());
        } else {
            published.unwrap();
        }
        let record = get_account_export_by_id(&pool, user, attempt.export)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            record.status,
            ExportStatus::Failed | ExportStatus::Cancelled
        ));
        let refs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM object_retained_references WHERE reference_id=$1",
        )
        .bind(attempt.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(refs, 0);
        assert!(store.get(attempt.key.as_str()).await.is_err());
    }
}

#[tokio::test]
async fn retry_releases_old_reference_and_fences_late_publication() {
    let Some((pool, user, _, _, store, _storage)) = setup().await else {
        return;
    };
    let (old, old_lease) = begin(&pool, user).await;
    let old_staging = zip::Staging::new().unwrap();
    let (old_inventory, old_until) = prepare_attempt(&pool, &old, &old_lease, old_staging.path())
        .await
        .unwrap();
    let old_summary = zip::package(&store, &old_inventory, old_staging.path(), user)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET leased_until=now()-interval '1 second' WHERE id=$1")
        .bind(old_lease.id)
        .execute(&pool)
        .await
        .unwrap();
    crate::jobs::recover_expired_jobs(&pool).await.unwrap();
    sqlx::query("UPDATE jobs SET available_at=now() WHERE id=$1")
        .bind(old_lease.id)
        .execute(&pool)
        .await
        .unwrap();
    let jobs =
        crate::jobs::claim_jobs(&pool, "retry", std::time::Duration::from_secs(3600), &[], 1)
            .await
            .unwrap();
    let lease = jobs[0].lease("retry");
    let fresh = start_attempt(&pool, old.export, &lease)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(old.id, fresh.id);
    let staging = zip::Staging::new().unwrap();
    let (inventory, until) = prepare_attempt(&pool, &fresh, &lease, staging.path())
        .await
        .unwrap();
    assert!(
        publish_attempt(
            &pool,
            &store,
            &old,
            &old_lease,
            &old_inventory,
            &old_summary,
            old_until
        )
        .await
        .is_err()
    );
    let summary = zip::package(&store, &inventory, staging.path(), user)
        .await
        .unwrap();
    publish_attempt(&pool, &store, &fresh, &lease, &inventory, &summary, until)
        .await
        .unwrap();
    assert!(
        !store
            .get(fresh.key.as_str())
            .await
            .unwrap()
            .meta
            .size
            .eq(&0)
    );
    assert!(store.get(old.key.as_str()).await.is_err());
    let mut tx = pool.begin().await.unwrap();
    lock_scope(&mut tx, user, &[]).await.unwrap();
    release_account_exports(&mut tx, user).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        get_account_export_by_id(&pool, user, old.export)
            .await
            .unwrap()
            .unwrap()
            .status,
        ExportStatus::Cancelled
    );
}

#[test]
fn staging_is_private_unique_and_removed_on_drop() {
    use std::os::unix::fs::PermissionsExt;
    let first = zip::Staging::new().unwrap();
    let second = zip::Staging::new().unwrap();
    assert_ne!(first.path(), second.path());
    assert_eq!(
        std::fs::metadata(first.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let path = first.path().to_owned();
    std::fs::write(path.join("partial"), b"partial").unwrap();
    drop(first);
    assert!(!path.exists());
}

#[tokio::test]
async fn existing_artifact_intent_cannot_be_adopted_or_overwritten() {
    let Some((pool, user, _, _, _, _storage)) = setup().await else {
        return;
    };
    let (attempt, lease) = begin(&pool, user).await;
    object_lifecycle::register_upload_intent(&pool, ObjectOwner::Account(user), &attempt.key)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE object_lifecycle SET retired_at=now(),deleted_at=now() WHERE storage_key=$1",
    )
    .bind(attempt.key.as_str())
    .execute(&pool)
    .await
    .unwrap();
    let staging = zip::Staging::new().unwrap();
    assert!(
        prepare_attempt(&pool, &attempt, &lease, staging.path())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn export_state_lock_does_not_block_job_heartbeat() {
    use sqlx::Connection;
    let Some((pool, user, _, _, _, _storage)) = setup().await else {
        return;
    };
    let (attempt, lease) = begin(&pool, user).await;
    let mut renewal = sqlx::PgConnection::connect_with(&pool.connect_options())
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    fence_attempt(&mut tx, &attempt, &lease).await.unwrap();
    let renewed=tokio::time::timeout(std::time::Duration::from_secs(2),
        sqlx::query("UPDATE jobs SET leased_until=now()+interval '1 hour',lease_renewed_at=now() WHERE id=$1 AND lease_owner=$2 AND claim_seq=$3")
            .bind(lease.id).bind(&lease.owner).bind(lease.claim_seq).execute(&mut renewal)).await;
    assert!(
        renewed.is_ok(),
        "collection/upload must allow lease renewal"
    );
    assert_eq!(renewed.unwrap().unwrap().rows_affected(), 1);
    tx.rollback().await.unwrap();
}
