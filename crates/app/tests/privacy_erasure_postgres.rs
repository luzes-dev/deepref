#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use bytes::Bytes;
use deepref_app::{
    account_deletion::{
        cancel_account_deletion, complete_account_deletions, request_account_deletion,
    },
    account_export::zip::Staging,
    identity::AuditRequest,
    object_lifecycle::cleanup_objects,
};
use deepref_documents::{DocumentStore, ObjectKey, StoreConfig};
use deepref_domain::UserId;
use sqlx::PgPool;
use uuid::Uuid;

const AUDIT: AuditRequest<'_> = AuditRequest {
    request_id: None,
    session_ref: None,
};
const DIGEST_KEY: &str = "fixture-only-deletion-hmac-key-not-a-deployed-secret";

struct RestorePermissions(std::path::PathBuf, std::fs::Permissions);
impl Drop for RestorePermissions {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.0, self.1.clone());
    }
}

struct Fixture {
    pool: PgPool,
    user: UserId,
    workspace: Uuid,
    document: Uuid,
    root: Staging,
    store: DocumentStore,
}
impl Fixture {
    async fn new() -> Option<Self> {
        let pool = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 1).await?;
        let user = UserId::new(Uuid::new_v4());
        sqlx::query("INSERT INTO users(id,workos_user_id,primary_email,verified_email,display_name,status) VALUES($1,$2,'erase@example.org','erase@example.org','Erase me','active')")
            .bind(user.as_uuid()).bind(user.as_uuid().to_string()).execute(&pool).await.unwrap();
        let workspace = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO workspaces(id,kind,name,slug) VALUES($1,'personal','Erasure fixture',$2)",
        )
        .bind(workspace)
        .bind(workspace.to_string())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO workspace_memberships(id,workspace_id,user_id,role,status,grant_source) VALUES($1,$2,$3,'owner','active','local')")
            .bind(Uuid::new_v4()).bind(workspace).bind(user.as_uuid()).execute(&pool).await.unwrap();
        let project = Uuid::new_v4();
        let document = Uuid::new_v4();
        sqlx::query("INSERT INTO projects(id,workspace_id,name) VALUES($1,$2,'Erasure science')")
            .bind(project)
            .bind(workspace)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO reports(id,title) VALUES($1,'Original evidence')")
            .bind(document)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
            .bind(project)
            .bind(document)
            .execute(&pool)
            .await
            .unwrap();
        let root = Staging::new().unwrap();
        let store = DocumentStore::new(StoreConfig::local(root.path()).unwrap(), 1024 * 1024);
        let key = ObjectKey::document(document);
        store
            .for_upload(key.clone())
            .put_stream(futures::stream::iter([Ok::<_, std::io::Error>(
                Bytes::from_static(b"evidence"),
            )]))
            .await
            .unwrap();
        sqlx::query("INSERT INTO documents(id,project_id,report_id,object_key,content_hash,mime_type,byte_size,status) VALUES($1,$2,$1,$3,encode(digest('evidence','sha256'),'hex'),'application/pdf',8,'uploaded')")
            .bind(document).bind(project).bind(key.as_str()).execute(&pool).await.unwrap();
        Some(Self {
            pool,
            user,
            workspace,
            document,
            root,
            store,
        })
    }
    fn object_path(&self) -> std::path::PathBuf {
        self.root
            .path()
            .join(ObjectKey::document(self.document).as_str())
    }
    async fn due(&self) {
        // Fresh disposable fixtures have no legacy writers/staging. The
        // baseline RED runs before the new migration; no imaginary public API
        // or missing-relation failure is substituted for the behavioral check.
        let gate: bool =
            sqlx::query_scalar("SELECT to_regclass('account_erasure_rollout') IS NOT NULL")
                .fetch_one(&self.pool)
                .await
                .unwrap();
        if gate {
            sqlx::query("UPDATE account_erasure_rollout SET activated=true,activated_at=clock_timestamp(),attestation='all-legacy-export-writers-stopped-and-untracked-staging-removed-no-restart' WHERE singleton")
                .execute(&self.pool).await.unwrap();
        }
        request_account_deletion(&self.pool, self.user, AUDIT)
            .await
            .unwrap();
        sqlx::query("UPDATE account_deletion_requests SET requested_at=clock_timestamp()-interval '31 days' WHERE user_id=$1")
            .bind(self.user.as_uuid()).execute(&self.pool).await.unwrap();
    }
    async fn complete(&self) -> deepref_app::account_deletion::CompletionReport {
        complete_account_deletions(&self.pool, DIGEST_KEY, AUDIT)
            .await
            .unwrap()
    }
}

async fn completion_pool(pool: &PgPool, application: &str, connections: u32) -> PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(connections)
        .connect_with(
            pool.connect_options()
                .as_ref()
                .clone()
                .application_name(application),
        )
        .await
        .unwrap()
}

async fn wait_for_completers(pool: &PgPool, applications: &[String], statement: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let blocked: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity WHERE datname=current_database()
                 AND application_name=ANY($1) AND wait_event_type='Lock' AND query LIKE $2",
            )
            .bind(applications)
            .bind(statement)
            .fetch_one(pool)
            .await
            .unwrap();
            if blocked == applications.len() as i64 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("all completers must reach the exact database lock boundary");
}

#[tokio::test]
async fn overlapping_completers_keep_one_proof_and_continue_to_later_accounts() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    f.due().await;
    // Keep the workspace as a deterministic pause after begin_account, but
    // acknowledge its real object first so finalization can remove progress.
    sqlx::query("DELETE FROM documents WHERE id=$1")
        .bind(f.document)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE object_lifecycle SET cleanup_after=clock_timestamp() WHERE storage_key=$1")
        .bind(ObjectKey::document(f.document).as_str())
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(cleanup_objects(&f.pool, &f.store, 1).await.unwrap(), 1);
    assert!(!f.object_path().exists());
    let later = UserId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO users(id,workos_user_id,primary_email,status) VALUES($1,$2,'later@example.org','active')")
        .bind(later.as_uuid()).bind(later.as_uuid().to_string()).execute(&f.pool).await.unwrap();
    request_account_deletion(&f.pool, later, AUDIT)
        .await
        .unwrap();
    sqlx::query("UPDATE account_deletion_requests SET requested_at=clock_timestamp()-interval '30 days 1 hour' WHERE user_id=$1")
        .bind(later.as_uuid()).execute(&f.pool).await.unwrap();
    let applications = [
        format!("completer-a-{}", f.user.as_uuid()),
        format!("completer-b-{}", f.user.as_uuid()),
    ];
    let control = completion_pool(&f.pool, "completion-control", 3).await;
    let a = completion_pool(&f.pool, &applications[0], 1).await;
    let b = completion_pool(&f.pool, &applications[1], 1).await;
    let mut workspace_block = control.begin().await.unwrap();
    sqlx::query("SELECT id FROM workspaces WHERE id=$1 FOR UPDATE")
        .bind(f.workspace)
        .execute(&mut *workspace_block)
        .await
        .unwrap();
    let first =
        tokio::spawn(async move { complete_account_deletions(&a, DIGEST_KEY, AUDIT).await });
    let second =
        tokio::spawn(async move { complete_account_deletions(&b, DIGEST_KEY, AUDIT).await });
    wait_for_completers(
        &control,
        &applications,
        "SELECT id FROM workspaces WHERE id = $1 FOR UPDATE",
    )
    .await;
    // Both passed progress creation. Queue both on finalization's request lock
    // before allowing either to publish a proof and delete that shared row.
    let mut request_block = control.begin().await.unwrap();
    sqlx::query("SELECT id FROM account_deletion_requests WHERE user_id=$1 FOR UPDATE")
        .bind(f.user.as_uuid())
        .execute(&mut *request_block)
        .await
        .unwrap();
    workspace_block.commit().await.unwrap();
    wait_for_completers(
        &control,
        &applications,
        "SELECT % FROM account_deletion_requests WHERE id = $1 FOR UPDATE",
    )
    .await;
    request_block.commit().await.unwrap();
    let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(first, second)
    })
    .await
    .expect("both completers must finish after the controlled release");
    let first = first.unwrap();
    let second = second.unwrap();
    assert!(
        first.is_ok() && second.is_ok(),
        "overlapping completion must be idempotent: {first:?}, {second:?}"
    );
    assert_eq!(first.unwrap().completed + second.unwrap().completed, 2);
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM fulfilled_account_deletions),
                (SELECT count(*) FROM admin_audit_events WHERE event_type='account.deletion_completed'),
                (SELECT count(*) FROM account_erasure_progress)",
    ).fetch_one(&control).await.unwrap();
    assert_eq!(
        counts,
        (2, 2, 0),
        "each account gets one proof and completion audit"
    );
    control.close().await;
}

#[tokio::test]
async fn denied_provider_callback_cannot_change_profile_after_deletion_capture() {
    use deepref_app::identity::{NewBrowserSession, SignIn, sign_in};
    use deepref_application::identity::{OpaqueSecret, ProviderProfile, ProviderSession};
    use deepref_domain::ExternalUserId;
    let Some(f) = Fixture::new().await else {
        return;
    };
    let other_workspace = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workspaces(id,kind,name,slug) VALUES($1,'organization','Invitation history',$2)",
    )
    .bind(other_workspace)
    .bind(other_workspace.to_string())
    .execute(&f.pool)
    .await
    .unwrap();
    let old_invite = Uuid::new_v4();
    let unrelated_invite = Uuid::new_v4();
    for (id, email) in [
        (old_invite, "erase@example.org"),
        (unrelated_invite, "callback-only@example.org"),
    ] {
        sqlx::query("INSERT INTO workspace_invitations(id,workspace_id,email,role,invited_by,status,expires_at) VALUES($1,$2,$3,'member',$4,'pending',clock_timestamp()+interval '1 day')")
            .bind(id).bind(other_workspace).bind(email).bind(f.user.as_uuid()).execute(&f.pool).await.unwrap();
    }
    f.due().await;
    let application = format!("capture-callback-{}", f.user.as_uuid());
    let control = completion_pool(&f.pool, "capture-control", 2).await;
    let runner = completion_pool(&f.pool, &application, 1).await;
    let mut workspace_block = control.begin().await.unwrap();
    sqlx::query("SELECT id FROM workspaces WHERE id=$1 FOR UPDATE")
        .bind(f.workspace)
        .execute(&mut *workspace_block)
        .await
        .unwrap();
    let completion =
        tokio::spawn(async move { complete_account_deletions(&runner, DIGEST_KEY, AUDIT).await });
    wait_for_completers(
        &control,
        &[application],
        "SELECT id FROM workspaces WHERE id = $1 FOR UPDATE",
    )
    .await;
    let captured: (String, i64) = sqlx::query_as(
        "SELECT u.status,(SELECT count(*) FROM account_erasure_progress WHERE user_id=u.id) FROM users u WHERE id=$1",
    ).bind(f.user.as_uuid()).fetch_one(&f.pool).await.unwrap();
    assert_eq!(captured, ("deactivated".to_owned(), 1));
    let sealed = OpaqueSecret::new("fixture sealed provider payload");
    let result = sign_in(
        &f.pool,
        &ProviderSession {
            session_id: "denied-callback".to_owned(),
            external_user_id: ExternalUserId::parse(f.user.as_uuid().to_string()).unwrap(),
        },
        &ProviderProfile {
            email: "callback-only@example.org".to_owned(),
            email_verified: true,
            display_name: Some("Denied profile update".to_owned()),
        },
        NewBrowserSession {
            token_hash: &[7; 32],
            sealed_provider_session: &sealed,
            session_ref: &[8; 32],
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        },
        AUDIT,
    )
    .await
    .unwrap();
    assert_eq!(result, SignIn::Deactivated);
    let after_callback: (String, Option<String>) =
        sqlx::query_as("SELECT primary_email,verified_email FROM users WHERE id=$1")
            .bind(f.user.as_uuid())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    workspace_block.commit().await.unwrap();
    completion.await.unwrap().unwrap();
    // A new registration's invitation is beyond the first logical scrub cutoff.
    let later_invite = Uuid::new_v4();
    sqlx::query("INSERT INTO workspace_invitations(id,workspace_id,email,role,invited_by,status,expires_at) VALUES($1,$2,'erase@example.org','member',$3,'pending',clock_timestamp()+interval '1 day')")
        .bind(later_invite).bind(other_workspace).bind(f.user.as_uuid()).execute(&f.pool).await.unwrap();
    let finished = deepref_app::account_deletion::complete_account_deletions_with_store(
        &f.pool, &f.store, DIGEST_KEY, AUDIT,
    )
    .await
    .unwrap();
    assert_eq!(finished.completed, 1);
    assert_eq!(
        after_callback,
        (
            "erase@example.org".to_owned(),
            Some("erase@example.org".to_owned())
        ),
        "denied callbacks cannot introduce an address outside the captured deletion digests"
    );
    let surviving: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM workspace_invitations WHERE id=ANY($1) ORDER BY id")
            .bind(vec![old_invite, unrelated_invite, later_invite])
            .fetch_all(&f.pool)
            .await
            .unwrap();
    assert!(!surviving.contains(&old_invite));
    assert!(surviving.contains(&unrelated_invite));
    assert!(surviving.contains(&later_invite));
    let proof: Vec<String> = sqlx::query_scalar(
        "SELECT email_hmac_sha256 FROM fulfilled_account_deletions WHERE user_id=$1",
    )
    .bind(f.user.as_uuid())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(proof.len(), 1);
    assert!(
        proof
            .iter()
            .all(|digest| digest.len() == 64 && !digest.contains('@'))
    );
    control.close().await;
}

#[tokio::test]
async fn full_thirty_day_cancellation_precedes_any_object_retirement() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    request_account_deletion(&f.pool, f.user, AUDIT)
        .await
        .unwrap();
    sqlx::query("UPDATE account_deletion_requests SET requested_at=clock_timestamp()-interval '29 days 23 hours' WHERE user_id=$1")
        .bind(f.user.as_uuid()).execute(&f.pool).await.unwrap();
    assert_eq!(f.complete().await.completed, 0);
    assert!(f.object_path().is_file());
    let retired: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT retired_at FROM object_lifecycle WHERE storage_key=$1")
            .bind(ObjectKey::document(f.document).as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(retired.is_none());
    cancel_account_deletion(&f.pool, f.user, AUDIT)
        .await
        .unwrap();
    f.due().await;
    let before = deepref_app::privacy_erasure::report(&f.pool).await.unwrap();
    assert_eq!(before.due_requests, 1);
    assert_eq!(before.pending_accounts, 0);
    assert!(
        deepref_app::account_deletion::due_accounts(&f.pool)
            .await
            .unwrap()
            .iter()
            .any(|a| a.user_id == f.user)
    );
    let first = f.complete().await;
    assert_eq!(first.physical_erasure.due_requests, 1);
    assert_eq!(first.physical_erasure.pending_accounts, 1);
    assert!(first.physical_erasure.oldest_account_due_at.is_some());
    // Public behavior RED on the reviewed baseline: metadata-only completion
    // incorrectly publishes its proof while the original bytes remain present.
    assert!(
        f.object_path().is_file(),
        "the original scientific bytes are still present at the proof boundary"
    );
    assert_eq!(
        first.completed, 0,
        "physical bytes remain; fulfillment must wait for an actual storage acknowledgement"
    );
    let state: String = sqlx::query_scalar(
        "SELECT state FROM account_deletion_requests WHERE user_id=$1 AND state<>'cancelled'",
    )
    .bind(f.user.as_uuid())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(state, "completing");
    let proof: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM fulfilled_account_deletions WHERE user_id=$1)",
    )
    .bind(f.user.as_uuid())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert!(!proof);
    let immediately_due:bool=sqlx::query_scalar("SELECT retired_at IS NOT NULL AND cleanup_after<=clock_timestamp() FROM object_lifecycle WHERE storage_key=$1")
        .bind(ObjectKey::document(f.document).as_str()).fetch_one(&f.pool).await.unwrap();
    assert!(
        immediately_due,
        "due privacy erasure must not add another24-hour object grace"
    );
    assert_eq!(cleanup_objects(&f.pool, &f.store, 100).await.unwrap(), 1);
    assert!(!f.object_path().exists());
    assert_eq!(f.complete().await.completed, 1);
    let verified: bool = sqlx::query_scalar(
        "SELECT physical_verified_at IS NOT NULL FROM fulfilled_account_deletions WHERE user_id=$1",
    )
    .bind(f.user.as_uuid())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert!(verified);
    assert_eq!(f.complete().await.completed, 0);
}

#[tokio::test]
async fn storage_refusal_stays_overdue_and_never_publishes_fulfillment() {
    use std::os::unix::fs::PermissionsExt;
    let Some(f) = Fixture::new().await else {
        return;
    };
    f.due().await;
    assert_eq!(f.complete().await.completed, 0);
    let directory = f.object_path().parent().unwrap().to_owned();
    let original = std::fs::metadata(&directory).unwrap().permissions();
    let restore = RestorePermissions(directory.clone(), original.clone());
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(cleanup_objects(&f.pool, &f.store, 1).await.unwrap(), 0);
    let pending:(bool,i64,bool)=sqlx::query_as("SELECT deleted_at IS NULL,cleanup_attempts,cleanup_after>clock_timestamp() FROM object_lifecycle WHERE storage_key=$1")
        .bind(ObjectKey::document(f.document).as_str()).fetch_one(&f.pool).await.unwrap();
    assert_eq!(pending, (true, 1, true));
    let retry_at: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT cleanup_after FROM object_lifecycle WHERE storage_key=$1")
            .bind(ObjectKey::document(f.document).as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    let report = f.complete().await;
    assert_eq!(report.completed, 0);
    let after_replay: (chrono::DateTime<chrono::Utc>, i64) = sqlx::query_as(
        "SELECT cleanup_after,cleanup_attempts FROM object_lifecycle WHERE storage_key=$1",
    )
    .bind(ObjectKey::document(f.document).as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        after_replay,
        (retry_at, 1),
        "logical replay must preserve the storage-error retry deadline"
    );
    let operator = deepref_app::account_deletion::complete_account_deletions_with_store(
        &f.pool, &f.store, DIGEST_KEY, AUDIT,
    )
    .await
    .unwrap();
    assert_eq!(operator.physical_objects_acknowledged, 0);
    let after_operator: (chrono::DateTime<chrono::Utc>, i64) = sqlx::query_as(
        "SELECT cleanup_after,cleanup_attempts FROM object_lifecycle WHERE storage_key=$1",
    )
    .bind(ObjectKey::document(f.document).as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(after_operator, (retry_at, 1));
    let overdue:i64=sqlx::query_scalar("SELECT count(*) FROM object_lifecycle WHERE privacy_due_at<clock_timestamp() AND deleted_at IS NULL")
        .fetch_one(&f.pool).await.unwrap();
    assert_eq!(overdue, 1);
    std::fs::set_permissions(&directory, original).unwrap();
    drop(restore);
    sqlx::query("UPDATE object_lifecycle SET cleanup_after=clock_timestamp()-interval '1 second' WHERE storage_key=$1")
        .bind(ObjectKey::document(f.document).as_str()).execute(&f.pool).await.unwrap();
    assert_eq!(cleanup_objects(&f.pool, &f.store, 1).await.unwrap(), 1);
    assert!(!f.object_path().exists());
    assert_eq!(f.complete().await.completed, 1);
}

#[tokio::test]
async fn account_archive_storage_failure_backoff_survives_completer_replay() {
    use deepref_app::object_lifecycle::{ObjectOwner, begin_object_upload};
    use std::os::unix::fs::PermissionsExt;
    let Some(f) = Fixture::new().await else {
        return;
    };
    // Isolate an account-owned archive: unlike deleted workspace metadata,
    // this ownership is reconsidered on every logical completion replay.
    sqlx::query("DELETE FROM documents WHERE id=$1")
        .bind(f.document)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE object_lifecycle SET cleanup_after=clock_timestamp() WHERE storage_key=$1")
        .bind(ObjectKey::document(f.document).as_str())
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(cleanup_objects(&f.pool, &f.store, 1).await.unwrap(), 1);
    f.due().await;
    let key = ObjectKey::account_export(f.user.as_uuid(), Uuid::new_v4());
    let upload = begin_object_upload(&f.pool, ObjectOwner::Account(f.user), &key)
        .await
        .unwrap();
    f.store
        .for_upload(key.clone())
        .put_stream(futures::stream::iter([Ok::<_, std::io::Error>(
            Bytes::from_static(b"private archive bytes"),
        )]))
        .await
        .unwrap();
    upload.commit().await.unwrap();
    assert_eq!(f.complete().await.completed, 0);
    let path = f.root.path().join(key.as_str());
    let directory = path.parent().unwrap().to_owned();
    let original = std::fs::metadata(&directory).unwrap().permissions();
    let restore = RestorePermissions(directory.clone(), original.clone());
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(cleanup_objects(&f.pool, &f.store, 1).await.unwrap(), 0);
    let before: (chrono::DateTime<chrono::Utc>, i64) = sqlx::query_as(
        "SELECT cleanup_after,cleanup_attempts FROM object_lifecycle WHERE storage_key=$1",
    )
    .bind(key.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(before.1, 1);
    assert_eq!(f.complete().await.completed, 0);
    let after: (chrono::DateTime<chrono::Utc>, i64) = sqlx::query_as(
        "SELECT cleanup_after,cleanup_attempts FROM object_lifecycle WHERE storage_key=$1",
    )
    .bind(key.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    std::fs::set_permissions(&directory, original).unwrap();
    drop(restore);
    assert_eq!(
        after, before,
        "account archive replay must preserve the storage-error retry deadline"
    );
    // Restoring the adapter does not authorize another attempt before backoff.
    let operator = deepref_app::account_deletion::complete_account_deletions_with_store(
        &f.pool, &f.store, DIGEST_KEY, AUDIT,
    )
    .await
    .unwrap();
    assert_eq!(operator.physical_objects_acknowledged, 0);
    assert!(path.is_file());
    let unchanged: (chrono::DateTime<chrono::Utc>, i64) = sqlx::query_as(
        "SELECT cleanup_after,cleanup_attempts FROM object_lifecycle WHERE storage_key=$1",
    )
    .bind(key.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(unchanged, before);
    sqlx::query("UPDATE object_lifecycle SET cleanup_after=clock_timestamp()-interval '1 second' WHERE storage_key=$1")
        .bind(key.as_str()).execute(&f.pool).await.unwrap();
    let due = deepref_app::account_deletion::complete_account_deletions_with_store(
        &f.pool, &f.store, DIGEST_KEY, AUDIT,
    )
    .await
    .unwrap();
    assert_eq!(due.physical_objects_acknowledged, 1);
    assert_eq!(due.completed, 1);
    assert!(!path.exists());
}

#[tokio::test]
async fn unknown_upload_is_not_resolved_by_a_later_missing_object() {
    use deepref_app::account_export::{
        EXPORT_ATTESTATION_TOKEN, activate_export_retention, process_export_job,
        request_account_export,
    };
    use std::os::unix::fs::PermissionsExt;
    let Some(f) = Fixture::new().await else {
        return;
    };
    activate_export_retention(&f.pool, EXPORT_ATTESTATION_TOKEN)
        .await
        .unwrap();
    let export = request_account_export(&f.pool, f.user, AUDIT)
        .await
        .unwrap();
    let jobs = deepref_app::claim_jobs(
        &f.pool,
        "failed-upload",
        std::time::Duration::from_secs(3600),
        &[],
        1,
    )
    .await
    .unwrap();
    assert_eq!(jobs.len(), 1);
    // Permit the source read while refusing the real archive upload. The
    // existing adapter exposes no definite multipart-abort receipt on error.
    let path = f.root.path().to_path_buf();
    let original = std::fs::metadata(&path).unwrap().permissions();
    let restore = RestorePermissions(path.clone(), original.clone());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o500)).unwrap();
    let result = process_export_job(
        &f.pool,
        &f.store,
        &serde_json::json!({"export_id":export.id}),
        &jobs[0].lease("failed-upload"),
    )
    .await;
    std::fs::set_permissions(&path, original).unwrap();
    drop(restore);
    assert!(result.is_err(), "the actual storage upload must be refused");
    let (stopped,outcome,directory):(bool,String,String)=sqlx::query_as("SELECT stopped_at IS NOT NULL,upload_outcome,staging_directory FROM account_export_writers WHERE export_id=$1")
        .bind(export.id).fetch_one(&f.pool).await.unwrap();
    assert!(stopped);
    assert_eq!(outcome, "unresolved");
    assert!(!std::path::Path::new(&directory).exists());
    f.due().await;
    assert_eq!(f.complete().await.completed, 0);
    assert_eq!(cleanup_objects(&f.pool, &f.store, 100).await.unwrap(), 2);
    let report = f.complete().await;
    assert_eq!(report.completed, 0);
    assert_eq!(report.physically_pending, 1);
    assert_eq!(report.physical_erasure.pending_objects, 0);
    assert_eq!(report.physical_erasure.unresolved_uploads, 1);
    let proofs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fulfilled_account_deletions WHERE user_id=$1")
            .bind(f.user.as_uuid())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(
        proofs, 0,
        "NotFound is not a multipart completion/abort receipt"
    );
}

#[tokio::test]
async fn rollout_requires_legacy_object_acknowledgements_and_fences_unverified_proofs() {
    use deepref_app::{
        object_lifecycle::{ObjectOwner, register_upload_intent},
        privacy_erasure::{ERASURE_DRAIN_ATTESTATION, activate, report},
    };
    let Some(f) = Fixture::new().await else {
        return;
    };
    let key = ObjectKey::account_export(f.user.as_uuid(), Uuid::new_v4());
    register_upload_intent(&f.pool, ObjectOwner::Account(f.user), &key)
        .await
        .unwrap();
    f.store
        .for_upload(key.clone())
        .put_stream(futures::stream::iter([Ok::<_, std::io::Error>(
            Bytes::from_static(b"legacy private copy"),
        )]))
        .await
        .unwrap();
    assert!(activate(&f.pool, ERASURE_DRAIN_ATTESTATION).await.is_err());
    assert_eq!(report(&f.pool).await.unwrap().unjournaled_legacy_objects, 1);
    sqlx::query("UPDATE object_lifecycle SET cleanup_after=clock_timestamp()-interval '1 second' WHERE storage_key=$1")
        .bind(key.as_str()).execute(&f.pool).await.unwrap();
    assert_eq!(cleanup_objects(&f.pool, &f.store, 100).await.unwrap(), 1);
    assert!(!f.root.path().join(key.as_str()).exists());
    activate(&f.pool, ERASURE_DRAIN_ATTESTATION).await.unwrap();
    assert!(
        sqlx::query(
            "INSERT INTO fulfilled_account_deletions(user_id,email_hmac_sha256) VALUES($1,'{}')"
        )
        .bind(f.user.as_uuid())
        .execute(&f.pool)
        .await
        .is_err(),
        "an old metadata-only writer cannot publish a new proof after activation"
    );
    f.due().await;
    assert_eq!(f.complete().await.completed, 0);
    assert!(sqlx::query("INSERT INTO fulfilled_account_deletions(user_id,email_hmac_sha256,physical_verified_at,logical_scrubbed_at) SELECT user_id,email_hmac_sha256,clock_timestamp(),logical_scrubbed_at FROM account_erasure_progress WHERE user_id=$1")
        .bind(f.user.as_uuid()).execute(&f.pool).await.is_err(), "a caller-supplied timestamp cannot bypass actual pending bytes");
    assert_eq!(cleanup_objects(&f.pool, &f.store, 100).await.unwrap(), 1);
    assert_eq!(f.complete().await.completed, 1);
    assert!(
        sqlx::query(
            "UPDATE fulfilled_account_deletions SET physical_verified_at=NULL WHERE user_id=$1"
        )
        .bind(f.user.as_uuid())
        .execute(&f.pool)
        .await
        .is_err()
    );
}

#[tokio::test]
async fn foreign_export_copy_is_cancelled_without_erasing_shared_workspace_sources() {
    use deepref_app::account_export::{
        EXPORT_ATTESTATION_TOKEN, ExportStatus, activate_export_retention, authorize_download,
        cancel_account_export, get_account_export_by_id, process_export_job,
        request_account_export,
    };
    let Some(f) = Fixture::new().await else {
        return;
    };
    let other = UserId::new(Uuid::new_v4());
    sqlx::query("INSERT INTO users(id,workos_user_id,primary_email,display_name,status) VALUES($1,$2,'other@example.org','Other researcher','active')")
        .bind(other.as_uuid()).bind(other.as_uuid().to_string()).execute(&f.pool).await.unwrap();
    sqlx::query("INSERT INTO workspace_memberships(id,workspace_id,user_id,role,status,grant_source) VALUES($1,$2,$3,'owner','active','local')")
        .bind(Uuid::new_v4()).bind(f.workspace).bind(other.as_uuid()).execute(&f.pool).await.unwrap();
    let shared = Uuid::new_v4();
    let project = Uuid::new_v4();
    let document = Uuid::new_v4();
    sqlx::query("INSERT INTO workspaces(id,kind,name,slug) VALUES($1,'organization','Co-owned scientific workspace',$2)")
        .bind(shared).bind(shared.to_string()).execute(&f.pool).await.unwrap();
    for user in [f.user, other] {
        sqlx::query("INSERT INTO workspace_memberships(id,workspace_id,user_id,role,status,grant_source) VALUES($1,$2,$3,'owner','active','local')")
            .bind(Uuid::new_v4()).bind(shared).bind(user.as_uuid()).execute(&f.pool).await.unwrap();
    }
    sqlx::query("INSERT INTO projects(id,workspace_id,name) VALUES($1,$2,'Shared provenance')")
        .bind(project)
        .bind(shared)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO reports(id,title) VALUES($1,'Shared evidence')")
        .bind(document)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
        .bind(project)
        .bind(document)
        .execute(&f.pool)
        .await
        .unwrap();
    let key = ObjectKey::document(document);
    f.store
        .for_upload(key.clone())
        .put_stream(futures::stream::iter([Ok::<_, std::io::Error>(
            Bytes::from_static(b"shared"),
        )]))
        .await
        .unwrap();
    sqlx::query("INSERT INTO documents(id,project_id,report_id,object_key,content_hash,mime_type,byte_size,status,actor_kind,actor_id) VALUES($1,$2,$1,$3,encode(digest('shared','sha256'),'hex'),'application/pdf',6,'uploaded','user',$4)")
        .bind(document).bind(project).bind(key.as_str()).bind(f.user.as_uuid().to_string()).execute(&f.pool).await.unwrap();
    activate_export_retention(&f.pool, EXPORT_ATTESTATION_TOKEN)
        .await
        .unwrap();
    let completed = request_account_export(&f.pool, other, AUDIT).await.unwrap();
    let claims = deepref_app::claim_jobs(
        &f.pool,
        "erasure-export-writer",
        std::time::Duration::from_secs(3600),
        &[],
        1,
    )
    .await
    .unwrap();
    assert_eq!(claims.len(), 1);
    process_export_job(
        &f.pool,
        &f.store,
        &serde_json::json!({"export_id":completed.id}),
        &claims[0].lease("erasure-export-writer"),
    )
    .await
    .unwrap();
    let legacy_copy = authorize_download(&f.pool, other, completed.id)
        .await
        .unwrap()
        .storage_key
        .unwrap();
    assert!(f.root.path().join(&legacy_copy).is_file());
    cancel_account_export(&f.pool, other, completed.id, AUDIT)
        .await
        .unwrap();
    // Seed a crash after a genuine ZIP was uploaded but before publication.
    // Its new request is still processing: terminal exports never restart.
    let archive = request_account_export(&f.pool, other, AUDIT).await.unwrap();
    let interrupted = deepref_app::claim_jobs(
        &f.pool,
        "interrupted-writer",
        std::time::Duration::from_secs(3600),
        &[],
        1,
    )
    .await
    .unwrap();
    assert_eq!(interrupted.len(), 1);
    let durable_job: Uuid =
        sqlx::query_scalar("SELECT job_id FROM account_data_exports WHERE id=$1")
            .bind(archive.id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(interrupted[0].id, durable_job);
    let old_attempt = Uuid::new_v4();
    let copy = ObjectKey::account_export(other.as_uuid(), old_attempt);
    deepref_app::object_lifecycle::register_upload_intent(
        &f.pool,
        deepref_app::object_lifecycle::ObjectOwner::Account(other),
        &copy,
    )
    .await
    .unwrap();
    let actual_archive = f.store.get(&legacy_copy).await.unwrap();
    f.store
        .for_upload(copy.clone())
        .put_stream(actual_archive.into_stream())
        .await
        .unwrap();
    assert!(f.root.path().join(copy.as_str()).is_file());
    let captured = vec![f.workspace, shared];
    sqlx::query("UPDATE account_data_exports SET status='processing',attempt_id=$2,claim_owner='interrupted-writer',claim_seq=$3,processing_started_at=clock_timestamp(),captured_workspaces=$4 WHERE id=$1")
        .bind(archive.id).bind(old_attempt).bind(interrupted[0].claim_seq).bind(&captured).execute(&f.pool).await.unwrap();
    sqlx::query("INSERT INTO account_export_writers(attempt_id,user_id,export_id,archive_key,captured_workspaces,writer_owner,claim_seq,upload_outcome,upload_intent_at,upload_acknowledged_at,stopped_at) VALUES($1,$2,$3,$4,$5,'interrupted-writer',$6,'acknowledged',clock_timestamp(),clock_timestamp(),clock_timestamp())")
        .bind(old_attempt).bind(other.as_uuid()).bind(archive.id).bind(copy.as_str()).bind(&captured).bind(interrupted[0].claim_seq).execute(&f.pool).await.unwrap();
    sqlx::query("INSERT INTO object_retained_references(reference_id,storage_key,expires_at) SELECT $1,d.object_key,clock_timestamp()+interval '7 days' FROM documents d JOIN projects p ON p.id=d.project_id WHERE p.workspace_id=ANY($2) AND d.object_key IS NOT NULL")
        .bind(old_attempt).bind(&captured).execute(&f.pool).await.unwrap();
    // The other researcher leaves the first workspace after capturing its
    // bytes. The account being deleted now solely owns only that workspace.
    sqlx::query("DELETE FROM workspace_memberships WHERE workspace_id=$1 AND user_id=$2")
        .bind(f.workspace)
        .bind(other.as_uuid())
        .execute(&f.pool)
        .await
        .unwrap();
    // Exercise a genuine second generation of the same request. Its latest
    // snapshot contains only the shared workspace, while the old uploaded copy
    // still contains the account's private workspace. Mutable request scope
    // alone must not make that old copy disappear from the erasure inventory.
    sqlx::query("UPDATE jobs SET leased_until=clock_timestamp()-interval '1 second' WHERE id=$1")
        .bind(interrupted[0].id)
        .execute(&f.pool)
        .await
        .unwrap();
    deepref_app::recover_expired_jobs(&f.pool).await.unwrap();
    sqlx::query("UPDATE jobs SET available_at=clock_timestamp() WHERE id=$1")
        .bind(interrupted[0].id)
        .execute(&f.pool)
        .await
        .unwrap();
    let retry = deepref_app::claim_jobs(
        &f.pool,
        "erasure-retry-writer",
        std::time::Duration::from_secs(3600),
        &[],
        1,
    )
    .await
    .unwrap();
    assert_eq!(retry.len(), 1);
    assert_eq!(retry[0].id, interrupted[0].id);
    assert!(retry[0].claim_seq > interrupted[0].claim_seq);
    process_export_job(
        &f.pool,
        &f.store,
        &serde_json::json!({"export_id":archive.id}),
        &retry[0].lease("erasure-retry-writer"),
    )
    .await
    .unwrap();
    let current = authorize_download(&f.pool, other, archive.id)
        .await
        .unwrap()
        .storage_key
        .unwrap();
    assert_ne!(copy.as_str(), current);
    let latest_scope: Vec<Uuid> =
        sqlx::query_scalar("SELECT captured_workspaces FROM account_data_exports WHERE id=$1")
            .bind(archive.id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(latest_scope, vec![shared]);
    f.due().await;
    assert_eq!(f.complete().await.completed, 0);
    assert_eq!(
        get_account_export_by_id(&f.pool, other, archive.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ExportStatus::Cancelled
    );
    assert!(
        authorize_download(&f.pool, other, archive.id)
            .await
            .is_err()
    );
    assert_eq!(cleanup_objects(&f.pool, &f.store, 100).await.unwrap(), 3);
    assert!(!f.object_path().exists());
    assert!(!f.root.path().join(copy.as_str()).exists());
    assert!(!f.root.path().join(&legacy_copy).exists());
    assert!(
        f.root.path().join(&current).is_file(),
        "the newer archive has no private workspace bytes and is not another tenant's erasure target"
    );
    assert!(f.root.path().join(key.as_str()).is_file());
    let actor: String = sqlx::query_scalar("SELECT actor_id FROM documents WHERE id=$1")
        .bind(document)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(actor, f.user.as_uuid().to_string());
    let retired: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT retired_at FROM object_lifecycle WHERE storage_key=$1")
            .bind(key.as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(retired.is_none());
    assert_eq!(f.complete().await.completed, 1);
}

#[tokio::test]
async fn later_physical_fulfillment_preserves_the_original_invitation_replay_cutoff() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    f.due().await;
    assert_eq!(f.complete().await.completed, 0);
    let cutoff: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT logical_scrubbed_at FROM account_erasure_progress WHERE user_id=$1",
    )
    .bind(f.user.as_uuid())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let inviter = Uuid::new_v4();
    let workspace = Uuid::new_v4();
    let invitation = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,workos_user_id,primary_email,display_name,status) VALUES($1,$2,'inviter@example.org','Inviter','active')")
        .bind(inviter).bind(inviter.to_string()).execute(&f.pool).await.unwrap();
    sqlx::query("INSERT INTO workspaces(id,kind,name,slug) VALUES($1,'organization','New account invitation',$2)")
        .bind(workspace).bind(workspace.to_string()).execute(&f.pool).await.unwrap();
    sqlx::query("INSERT INTO workspace_memberships(id,workspace_id,user_id,role,status,grant_source) VALUES($1,$2,$3,'owner','active','local')")
        .bind(Uuid::new_v4()).bind(workspace).bind(inviter).execute(&f.pool).await.unwrap();
    let created: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("INSERT INTO workspace_invitations(id,workspace_id,email,role,invited_by,status,expires_at) VALUES($1,$2,'erase@example.org','member',$3,'pending',clock_timestamp()+interval '1 day') RETURNING created_at")
        .bind(invitation).bind(workspace).bind(inviter).fetch_one(&f.pool).await.unwrap();
    assert!(
        created > cutoff,
        "the new invitation must be after logical anonymization"
    );
    assert_eq!(cleanup_objects(&f.pool, &f.store, 100).await.unwrap(), 1);
    assert_eq!(f.complete().await.completed, 1);
    let (logical,physical):(chrono::DateTime<chrono::Utc>,chrono::DateTime<chrono::Utc>)=sqlx::query_as("SELECT logical_scrubbed_at,physical_verified_at FROM fulfilled_account_deletions WHERE user_id=$1")
        .bind(f.user.as_uuid()).fetch_one(&f.pool).await.unwrap();
    assert_eq!(logical, cutoff);
    assert!(physical > created);
    // Restore replay must retain the earlier cutoff, rather than scrubbing
    // invitations for a re-registration while the physical step was pending.
    assert_eq!(f.complete().await.completed, 0);
    let preserved: (String, String) =
        sqlx::query_as("SELECT email,status FROM workspace_invitations WHERE id=$1")
            .bind(invitation)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(preserved, ("erase@example.org".into(), "pending".into()));
}
