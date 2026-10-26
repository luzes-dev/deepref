#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use bytes::Bytes;
use deepref_app::{account_export::*, identity::AuditRequest};
use deepref_application::jobs::JobLease;
use deepref_documents::{DocumentStore, ObjectKey, StoreConfig};
use deepref_domain::UserId;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::time::Duration;
use uuid::Uuid;

use deepref_app as jobs;

const AUDIT: AuditRequest<'_> = AuditRequest {
    request_id: None,
    session_ref: None,
};
struct Fixture {
    pool: PgPool,
    user: UserId,
    workspace: Uuid,
    project: Uuid,
    store: DocumentStore,
    dir: zip::Staging,
}
impl Fixture {
    async fn new() -> Option<Self> {
        // Every test exercises the sole-connection path, not just an isolated helper.
        let pool = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 1).await?;
        let user = UserId::new(Uuid::new_v4());
        sqlx::query("INSERT INTO users(id,workos_user_id,primary_email,display_name,status) VALUES($1,'SECRET_WORKOS_SENTINEL','export@example.org','Exporter','active')")
            .bind(user.as_uuid()).execute(&pool).await.unwrap();
        let workspace = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO workspaces(id,kind,name,slug) VALUES($1,'personal','Owned','owned')",
        )
        .bind(workspace)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO workspace_memberships(id,workspace_id,user_id,role,status,grant_source) VALUES($1,$2,$3,'owner','active','local')")
            .bind(Uuid::new_v4()).bind(workspace).bind(user.as_uuid()).execute(&pool).await.unwrap();
        let project = Uuid::new_v4();
        sqlx::query("INSERT INTO projects(id,name,workspace_id) VALUES($1,'Research',$2)")
            .bind(project)
            .bind(workspace)
            .execute(&pool)
            .await
            .unwrap();
        let dir = zip::Staging::new().unwrap();
        let store = DocumentStore::new(
            StoreConfig::local(dir.path()).unwrap(),
            MAX_ARCHIVE_BYTES as usize,
        );
        Some(Self {
            pool,
            user,
            workspace,
            project,
            store,
            dir,
        })
    }
    async fn request(&self) -> AccountExportRecord {
        activate_export_retention(&self.pool, EXPORT_ATTESTATION_TOKEN)
            .await
            .unwrap();
        request_account_export(&self.pool, self.user, AUDIT)
            .await
            .unwrap()
    }
    async fn claim(&self) -> JobLease {
        let jobs = jobs::claim_jobs(&self.pool, "export-test", Duration::from_secs(3600), &[], 1)
            .await
            .unwrap();
        assert_eq!(jobs.len(), 1);
        jobs[0].lease("export-test")
    }
    async fn process(&self, export: Uuid, lease: &JobLease) -> anyhow::Result<()> {
        tokio::time::timeout(
            Duration::from_secs(30),
            process_export_job(
                &self.pool,
                &self.store,
                &serde_json::json!({"export_id":export}),
                lease,
            ),
        )
        .await
        .unwrap()
    }
    async fn document(&self, bytes: &[u8]) -> (Uuid, ObjectKey) {
        let id = Uuid::new_v4();
        let key = ObjectKey::document(id);
        self.store
            .for_upload(key.clone())
            .put_stream(futures::stream::iter(
                bytes
                    .chunks(8191)
                    .map(|chunk| Ok::<_, std::io::Error>(Bytes::copy_from_slice(chunk)))
                    .collect::<Vec<_>>(),
            ))
            .await
            .unwrap();
        sqlx::query("INSERT INTO reports(id,title) VALUES($1,'Original')")
            .bind(id)
            .execute(&self.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
            .bind(self.project)
            .bind(id)
            .execute(&self.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO documents(id,report_id,project_id,object_key,content_hash,mime_type,byte_size,original_filename,status) VALUES($1,$1,$2,$3,$4,'application/pdf',$5,'../../unsafe.pdf','uploaded')")
            .bind(id).bind(self.project).bind(key.as_str()).bind(Sha256::digest(bytes).as_slice().iter().map(|byte| format!("{byte:02x}")).collect::<String>()).bind(bytes.len() as i64).execute(&self.pool).await.unwrap();
        (id, key)
    }
    async fn inline(&self, bytes: &[u8]) -> Uuid {
        let workflow = Uuid::new_v4();
        let run = Uuid::new_v4();
        let file = Uuid::new_v4();
        sqlx::query("INSERT INTO workflows(id,project_id,name,webhook_token,webhook_secret,email_token,created_by_kind,created_by_id) VALUES($1,$2,'Legacy','SECRET_WEBHOOK_TOKEN','SECRET_WEBHOOK_SECRET','SECRET_EMAIL_TOKEN','system','test')")
            .bind(workflow).bind(self.project).execute(&self.pool).await.unwrap();
        sqlx::query("INSERT INTO durable_runs(id,project_id,kind,actor_kind,actor_id,idempotency_key,status) VALUES($1,$2,'workflow','system','test',$3,'queued')")
            .bind(run).bind(self.project).bind(run.to_string()).execute(&self.pool).await.unwrap();
        sqlx::query("INSERT INTO workflow_files(id,project_id,run_id,node_id,name,content_type,content) VALUES($1,$2,$3,'node','../../legacy.csv','text/csv',$4)")
            .bind(file).bind(self.project).bind(run).bind(bytes).execute(&self.pool).await.unwrap();
        file
    }
    async fn record(&self, id: Uuid) -> AccountExportRecord {
        get_account_export_by_id(&self.pool, self.user, id)
            .await
            .unwrap()
            .unwrap()
    }
    async fn references(&self, id: Uuid) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM object_retained_references WHERE reference_id=(SELECT attempt_id FROM account_data_exports WHERE id=$1)")
            .bind(id).fetch_one(&self.pool).await.unwrap()
    }
}

#[tokio::test]
async fn operator_attestation_gate() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    assert!(!is_export_retention_activated(&f.pool).await.unwrap());
    assert!(matches!(
        request_account_export(&f.pool, f.user, AUDIT).await,
        Err(ExportError::RetentionNotActivated)
    ));
    assert!(
        activate_export_retention(&f.pool, "all-eager-deleting-binaries-drained")
            .await
            .is_err()
    );
    assert!(sqlx::query("UPDATE account_export_rollout SET retention_activated=true,activated_at=now() WHERE singleton").execute(&f.pool).await.is_err(),"NULL must not bypass the CHECK");
    let export = f.request().await;
    assert_eq!(export.status, ExportStatus::Requested);
    assert!(matches!(
        request_account_export(&f.pool, f.user, AUDIT).await,
        Err(ExportError::ActiveExportExists)
    ));
    let attestation: String =
        sqlx::query_scalar("SELECT operator_attestation FROM account_export_rollout")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(attestation, EXPORT_ATTESTATION_TOKEN);
}

#[tokio::test]
async fn exact_multichunk_zip_inline_and_secret_exclusions_with_one_connection() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let bytes: Vec<u8> = (0..300_123).map(|n| (n % 251) as u8).collect();
    let (document, _) = f.document(&bytes).await;
    let inline = b"name,value\nlegacy,exact bytes\n";
    let workflow_file = f.inline(inline).await;
    sqlx::query("INSERT INTO api_keys(id,workspace_id,created_by_user_id,name,prefix,secret_hash,scopes) VALUES($1,$2,$3,'SECRET_KEY_SENTINEL','dr_key_v1_',decode(repeat('ab',32),'hex'),ARRAY['project:view'])")
        .bind(Uuid::new_v4()).bind(f.workspace).bind(f.user.as_uuid()).execute(&f.pool).await.unwrap();
    sqlx::query("INSERT INTO user_sessions(id,user_id,session_token_hash,sealed_provider_session,session_ref,expires_at) VALUES($1,$2,decode(repeat('cd',32),'hex'),'SECRET_COOKIE_REFRESH_TOKEN',decode(repeat('ef',32),'hex'),now()+interval '1 day')")
        .bind(Uuid::new_v4()).bind(f.user.as_uuid()).execute(&f.pool).await.unwrap();
    sqlx::query("INSERT INTO records(id,project_id,source,raw) VALUES($1,$2,'test','{\"credential\":\"SECRET_RAW_CONNECTOR\"}')")
        .bind(Uuid::new_v4()).bind(f.project).execute(&f.pool).await.unwrap();
    sqlx::query("INSERT INTO billing_accounts(id,workspace_id,provider_scope,operation_id,customer_ref,provisioning,projection) VALUES($1,$2,'dodo:test',$3,'SECRET_CUSTOMER_REFERENCE','mapped','{\"kind\":\"unknown\",\"internal_retention_note\":\"SECRET_FINANCE_NOTE\"}')")
        .bind(Uuid::new_v4()).bind(f.workspace).bind(Uuid::new_v4()).execute(&f.pool).await.unwrap();
    let export = f.request().await;
    let lease = f.claim().await;
    f.process(export.id, &lease).await.unwrap();
    let result = authorize_download(&f.pool, f.user, export.id)
        .await
        .unwrap();
    assert_eq!(result.status, ExportStatus::Completed);
    assert_eq!(f.references(export.id).await, 2);
    let artifact = f.dir.path().join(result.storage_key.unwrap());
    let expected = f.dir.path().join("expected.bin");
    std::fs::write(&expected, &bytes).unwrap();
    let output = std::process::Command::new("python3")
        .arg("-c")
        .arg(
            r#"
import sys,zipfile,json,hashlib
with zipfile.ZipFile(sys.argv[1]) as z:
 assert z.testzip() is None
 assert z.read('documents/'+sys.argv[3]) == open(sys.argv[2],'rb').read()
 assert z.read('workflow_files/'+sys.argv[4]) == b'name,value\nlegacy,exact bytes\n'
 assert all('..' not in name.split('/') and not name.startswith('/') for name in z.namelist())
 assert b'SECRET_' not in b''.join(z.read(name) for name in z.namelist())
 manifest=json.loads(z.read('manifest.json'))
 assert manifest['is_partial'] and manifest['exclusions']
 print(hashlib.sha256(open(sys.argv[1],'rb').read()).hexdigest())
"#,
        )
        .arg(&artifact)
        .arg(expected)
        .arg(document.to_string())
        .arg(workflow_file.to_string())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        result.sha256.unwrap()
    );
    cancel_account_export(&f.pool, f.user, export.id, AUDIT)
        .await
        .unwrap();
    assert_eq!(f.references(export.id).await, 0);
    f.process(export.id, &lease).await.unwrap();
    assert_eq!(f.record(export.id).await.status, ExportStatus::Cancelled);
}

#[tokio::test]
async fn missing_corrupt_and_oversize_sources_never_complete() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (document, key) = f.document(b"original bytes").await;
    for mode in ["missing", "hash", "length", "budget"] {
        match mode {
            "missing" => {
                std::fs::remove_file(f.dir.path().join(key.as_str())).unwrap();
            }
            "hash" => {
                std::fs::write(f.dir.path().join(key.as_str()), b"different byte").unwrap();
            }
            "length" => {
                std::fs::write(
                    f.dir.path().join(key.as_str()),
                    b"too long to match the recorded file length",
                )
                .unwrap();
            }
            "budget" => {
                sqlx::query("UPDATE documents SET byte_size=$2 WHERE id=$1")
                    .bind(document)
                    .bind(collector::MAX_SOURCE_BYTES as i64 + 1)
                    .execute(&f.pool)
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let export = f.request().await;
        let lease = f.claim().await;
        assert!(f.process(export.id, &lease).await.is_err(), "{mode}");
        let result = f.record(export.id).await;
        assert_eq!(result.status, ExportStatus::Failed, "{mode}");
        assert!(result.storage_key.is_none());
        assert_eq!(f.references(export.id).await, 0);
        jobs::complete_job(&f.pool, &lease).await.unwrap();
    }
}

#[tokio::test]
async fn global_row_and_json_limits_fail_instead_of_truncating() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    sqlx::query("INSERT INTO records(id,project_id,source,title) SELECT gen_random_uuid(),$1,'test','row' FROM generate_series(1,$2)")
        .bind(f.project).bind(collector::MAX_ROWS as i32+1).execute(&f.pool).await.unwrap();
    let export = f.request().await;
    let lease = f.claim().await;
    assert!(f.process(export.id, &lease).await.is_err());
    assert_eq!(f.record(export.id).await.status, ExportStatus::Failed);
    jobs::complete_job(&f.pool, &lease).await.unwrap();
    sqlx::query("DELETE FROM records WHERE project_id=$1")
        .bind(f.project)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE projects SET description=repeat('x',$2) WHERE id=$1")
        .bind(f.project)
        .bind(collector::MAX_ROW_BYTES + 1)
        .execute(&f.pool)
        .await
        .unwrap();
    let export = f.request().await;
    let lease = f.claim().await;
    assert!(f.process(export.id, &lease).await.is_err());
    assert_eq!(f.record(export.id).await.status, ExportStatus::Failed);
}

#[tokio::test]
async fn live_ownership_deactivation_foreign_ids_and_expiry_release() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let export = f.request().await;
    let lease = f.claim().await;
    f.process(export.id, &lease).await.unwrap();
    assert!(matches!(
        authorize_download(&f.pool, UserId::new(Uuid::new_v4()), export.id).await,
        Err(ExportError::NotFound)
    ));
    sqlx::query("UPDATE workspace_memberships SET role='member' WHERE user_id=$1")
        .bind(f.user.as_uuid())
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(matches!(
        authorize_download(&f.pool, f.user, export.id).await,
        Err(ExportError::NotFound)
    ));
    assert_eq!(f.references(export.id).await, 0);
    jobs::complete_job(&f.pool, &lease).await.unwrap();
    sqlx::query("UPDATE workspace_memberships SET role='owner' WHERE user_id=$1")
        .bind(f.user.as_uuid())
        .execute(&f.pool)
        .await
        .unwrap();
    let export = f.request().await;
    let lease = f.claim().await;
    f.process(export.id, &lease).await.unwrap();
    let key = f.record(export.id).await.storage_key.unwrap();
    sqlx::query(
        "UPDATE account_data_exports SET retained_until=now()-interval '1 second' WHERE id=$1",
    )
    .bind(export.id)
    .execute(&f.pool)
    .await
    .unwrap();
    expire_exports(&f.pool).await.unwrap();
    assert_eq!(f.record(export.id).await.status, ExportStatus::Expired);
    assert_eq!(f.references(export.id).await, 0);
    sqlx::query(
        "UPDATE object_lifecycle SET cleanup_after=now()-interval '1 second' WHERE storage_key=$1",
    )
    .bind(&key)
    .execute(&f.pool)
    .await
    .unwrap();
    deepref_app::object_lifecycle::cleanup_objects(&f.pool, &f.store, 100)
        .await
        .unwrap();
    assert!(f.store.get(&key).await.is_err());
    let retired:bool=sqlx::query_scalar("SELECT retired_at IS NOT NULL AND deleted_at IS NOT NULL FROM object_lifecycle WHERE storage_key=$1").bind(&key).fetch_one(&f.pool).await.unwrap();
    assert!(retired);
    jobs::complete_job(&f.pool, &lease).await.unwrap();
    let export = f.request().await;
    let lease = f.claim().await;
    f.process(export.id, &lease).await.unwrap();
    sqlx::query("UPDATE users SET status='deactivated' WHERE id=$1")
        .bind(f.user.as_uuid())
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(matches!(
        authorize_download(&f.pool, f.user, export.id).await,
        Err(ExportError::NotFound)
    ));
    assert_eq!(f.references(export.id).await, 0);
}

#[tokio::test]
async fn crashed_claim_gets_fresh_attempt_and_old_lease_cannot_publish() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let export = f.request().await;
    let old = f.claim().await;
    let abandoned = Uuid::new_v4();
    sqlx::query("UPDATE account_data_exports SET status='processing',attempt_id=$2,claim_owner=$3,claim_seq=$4 WHERE id=$1")
        .bind(export.id).bind(abandoned).bind(&old.owner).bind(old.claim_seq).execute(&f.pool).await.unwrap();
    sqlx::query("UPDATE jobs SET leased_until=now()-interval '1 second' WHERE id=$1")
        .bind(old.id)
        .execute(&f.pool)
        .await
        .unwrap();
    jobs::recover_expired_jobs(&f.pool).await.unwrap();
    sqlx::query("UPDATE jobs SET available_at=now() WHERE id=$1")
        .bind(old.id)
        .execute(&f.pool)
        .await
        .unwrap();
    let fresh = f.claim().await;
    assert!(fresh.claim_seq > old.claim_seq);
    assert!(f.process(export.id, &old).await.is_err());
    f.process(export.id, &fresh).await.unwrap();
    let attempt: Uuid =
        sqlx::query_scalar("SELECT attempt_id FROM account_data_exports WHERE id=$1")
            .bind(export.id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_ne!(attempt, abandoned);
    assert_eq!(f.record(export.id).await.status, ExportStatus::Completed);
}

#[tokio::test]
async fn old_worker_limit_progress_and_recovery_are_shielded() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let export = f.request().await;
    let ordinary = Uuid::new_v4();
    sqlx::query("INSERT INTO jobs(id,scope,kind,workload_class,payload,state,available_at) VALUES($1,'platform','recompute_metrics','projection','{}','queued',now())")
        .bind(ordinary).execute(&f.pool).await.unwrap();
    let selected: Uuid = sqlx::query_scalar(
        "SELECT id FROM jobs WHERE state='queued' ORDER BY available_at,id LIMIT 1",
    )
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(selected, ordinary);
    let row=sqlx::query("SELECT j.id,j.state FROM jobs j JOIN account_data_exports e ON e.job_id=j.id WHERE e.id=$1").bind(export.id).fetch_one(&f.pool).await.unwrap();
    assert_eq!(row.get::<String, _>("state"), "capability_wait");
    let export_job: Uuid = row.get("id");
    assert_eq!(sqlx::query("UPDATE jobs SET state='running',lease_owner='old',leased_until=now()+interval '1 hour' WHERE id=$1").bind(export_job).execute(&f.pool).await.unwrap().rows_affected(),0);
    let lease = f.claim().await;
    assert_eq!(lease.id, export_job);
    sqlx::query("UPDATE jobs SET leased_until=now()-interval '1 second' WHERE id=$1")
        .bind(export_job)
        .execute(&f.pool)
        .await
        .unwrap();
    let recovered=sqlx::query("UPDATE jobs SET state='queued',lease_owner=NULL,leased_until=NULL WHERE state='running' AND leased_until<now() RETURNING id").fetch_all(&f.pool).await.unwrap();
    assert!(
        recovered.is_empty(),
        "an old decoder must not recover the protected job"
    );
    jobs::recover_expired_jobs(&f.pool).await.unwrap();
    let protected: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id=$1")
        .bind(export_job)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(protected, "capability_wait");
}

#[tokio::test]
async fn scope_includes_sole_and_coowned_but_not_member_or_foreign() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let other = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,workos_user_id,primary_email,status) VALUES($1,'other-owner','other@example.org','active')").bind(other).execute(&f.pool).await.unwrap();
    let mut expected = vec![f.workspace];
    for role in [Some("owner"), Some("member"), None] {
        let workspace = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO workspaces(id,kind,name,slug) VALUES($1,'organization','Scope',$2)",
        )
        .bind(workspace)
        .bind(workspace.to_string())
        .execute(&f.pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO workspace_memberships(id,workspace_id,user_id,role,status,grant_source) VALUES($1,$2,$3,'owner','active','local')").bind(Uuid::new_v4()).bind(workspace).bind(other).execute(&f.pool).await.unwrap();
        if let Some(role) = role {
            sqlx::query("INSERT INTO workspace_memberships(id,workspace_id,user_id,role,status,grant_source) VALUES($1,$2,$3,$4,'active','local')").bind(Uuid::new_v4()).bind(workspace).bind(f.user.as_uuid()).bind(role).execute(&f.pool).await.unwrap();
            if role == "owner" {
                expected.push(workspace);
            }
        }
    }
    let export = f.request().await;
    let lease = f.claim().await;
    f.process(export.id, &lease).await.unwrap();
    let actual: Vec<Uuid> =
        sqlx::query_scalar("SELECT captured_workspaces FROM account_data_exports WHERE id=$1")
            .bind(export.id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    expected.sort();
    assert_eq!(actual, expected);
}

#[tokio::test]
async fn global_file_count_fails_before_reading_any_missing_object() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    sqlx::query("INSERT INTO reports(id,title) SELECT gen_random_uuid(),'Many files' FROM generate_series(1,$1)").bind(collector::MAX_FILES as i32+1).execute(&f.pool).await.unwrap();
    sqlx::query("INSERT INTO project_reports(project_id,report_id) SELECT $1,id FROM reports")
        .bind(f.project)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO documents(id,report_id,project_id,object_key,content_hash,mime_type,byte_size,status) SELECT id,id,$1,'documents/'||id::text,encode(digest(id::text,'sha256'),'hex'),'application/pdf',1,'uploaded' FROM reports")
        .bind(f.project).execute(&f.pool).await.unwrap();
    let export = f.request().await;
    let lease = f.claim().await;
    let error = f.process(export.id, &lease).await.unwrap_err();
    assert!(error.to_string().contains("source budget"), "{error:#}");
    assert_eq!(f.record(export.id).await.status, ExportStatus::Failed);
}
