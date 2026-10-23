#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
use std::path::PathBuf;

use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sqlx::{Connection, PgPool};
use uuid::Uuid;

use super::{
    Report,
    config::{Config, Limits, reject_symlinks, validate_key},
    db,
    inventory::{Entry, Inventory},
    objects,
};

struct Fixture {
    root: PathBuf,
    config: Config,
    inventory: Inventory,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("deepref-restore-{}", Uuid::new_v4())); // nosemgrep: rust.lang.security.temp-dir.temp-dir
        std::fs::create_dir_all(&root).unwrap();
        let config = Config {
            database_url_env: "DEEPREF_RESTORE_DATABASE_URL".into(),
            quarantine_root: root.clone(),
            inventory: root.join("inventory.json"),
            restore_target: Utc::now() - Duration::hours(1),
            stale_before: Utc::now() - Duration::minutes(15),
            limits: Limits {
                rows_per_table: 100,
                page_rows: 2,
                inventory_bytes: 65536,
                inventory_rows: 100,
                hash_samples: 10,
                hash_object_bytes: 1024,
                hash_total_bytes: 4096,
                statement_timeout_ms: 5000,
                overall_timeout_ms: 30000,
            },
        };
        let inventory = Inventory {
            schema_version: 1,
            snapshot_id: "fixture".into(),
            captured_at: Utc::now(),
            restore_target: config.restore_target,
            selection: "materialized_as_of".into(),
            complete: true,
            entries: Vec::new(),
        };
        Self {
            root,
            config,
            inventory,
        }
    }
    fn object(&mut self, key: &str, content: Option<&[u8]>) {
        let file = self.root.join(key);
        if let Some(content) = content {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, content).unwrap();
        }
        self.inventory.entries.push(Entry {
            key: key.into(),
            byte_size: content.map_or(3, |b| b.len() as u64),
            version_id: Some("selected-v1".into()),
        });
    }
    fn report(&self) -> Report {
        let mut report = Report::new();
        report.inventory = Some(json!({"operator_attested_complete":true}));
        for table in [
            "documents",
            "workflow_files",
            "jobs",
            "inbox",
            "outbox",
            "reservations",
        ] {
            report.database.scans.insert(
                table.into(),
                db::Scan {
                    rows: 0,
                    complete: true,
                },
            );
            report.database.records.insert(table.into(), Vec::new());
        }
        report
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}
fn reference(id: Uuid, key: &str, bytes: &[u8]) -> Value {
    json!({"id":id,"key":key,"sha256":objects::digest(bytes),"byte_size":bytes.len()})
}
fn local_deadline() -> super::deadline::Deadline {
    super::deadline::Deadline::after(std::time::Duration::from_secs(30))
}

fn codes(report: &Report) -> Vec<&str> {
    report
        .findings
        .iter()
        .filter_map(|f| f["code"].as_str())
        .collect()
}

#[test]
fn refuses_unsafe_config_inventory_paths_and_duplicates() {
    let mut f = Fixture::new();
    f.config.validate().unwrap();
    for key in [
        "",
        "/etc/passwd",
        "../outside",
        "a/../b",
        "a//b",
        "a/./b",
        "a\\b",
        "a%2fb",
        "C:/file",
        "a\n",
    ] {
        assert!(validate_key(key).is_err(), "{key:?}");
    }
    f.config.database_url_env = "DATABASE_URL".into();
    assert_eq!(f.config.validate(), Err("unsafe_or_invalid_config"));
    f.config.database_url_env = "DEEPREF_RESTORE_DATABASE_URL".into();
    f.object("objects/a", Some(b"abc"));
    f.object("objects/a", Some(b"abc"));
    assert_eq!(
        f.inventory.validate(&f.config, &local_deadline()),
        Err("inventory_duplicate_key")
    );
    f.inventory.entries.pop();
    f.inventory.restore_target -= Duration::seconds(1);
    assert_eq!(
        f.inventory.validate(&f.config, &local_deadline()),
        Err("inventory_contract_invalid_or_limit")
    );
    std::fs::write(&f.config.inventory, b"{}").unwrap();
    assert_eq!(
        super::config::read_json::<Value>(&f.config.inventory, 1, None),
        Err("input_byte_limit")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn refuses_symlink_materializations_and_unsafe_database_keys() {
    let mut f = Fixture::new();
    std::os::unix::fs::symlink("/etc", f.root.join("escape")).unwrap();
    assert_eq!(
        reject_symlinks(&f.root.join("escape/passwd"), None),
        Err("symlink_refused")
    );
    f.inventory.entries.push(Entry {
        key: "escape/passwd".into(),
        byte_size: 1,
        version_id: None,
    });
    let mut report = f.report();
    assert_eq!(
        objects::scan(&f.config, &f.inventory, &mut report, &local_deadline()).await,
        Err("symlink_refused")
    );
    assert_eq!(report.finish(), 2);
    f.inventory.entries.clear();
    report = f.report();
    report
        .database
        .records
        .get_mut("documents")
        .unwrap()
        .push(reference(Uuid::new_v4(), "../secret", b"abc"));
    assert_eq!(
        objects::scan(&f.config, &f.inventory, &mut report, &local_deadline()).await,
        Err("unsafe_object_key")
    );
    assert_eq!(report.finish(), 2);
}

#[tokio::test]
async fn detects_missing_versions_orphans_hashes_and_inline_compatibility() {
    let mut f = Fixture::new();
    f.object("objects/corrupt", Some(b"bad"));
    f.object("objects/orphan", Some(b"abc"));
    f.object("objects/missing", None);
    let mut r = f.report();
    let docs = r.database.records.get_mut("documents").unwrap();
    docs.push(reference(Uuid::from_u128(1), "objects/corrupt", b"abc"));
    docs.push(reference(Uuid::from_u128(2), "objects/missing", b"abc"));
    docs.push(reference(
        Uuid::from_u128(3),
        "objects/not-in-inventory",
        b"abc",
    ));
    r.database.records.get_mut("workflow_files").unwrap().push(
        json!({"id":Uuid::new_v4(),"key":null,"sha256":null,"byte_size":null,"inline_bytes":7}),
    );
    objects::scan(&f.config, &f.inventory, &mut r, &local_deadline())
        .await
        .unwrap();
    for code in [
        "sampled_hash_mismatch",
        "selected_version_materialization_missing",
        "inventory_orphan",
        "reference_absent_from_inventory",
    ] {
        assert!(codes(&r).contains(&code), "missing {code}");
    }
    assert_eq!(r.objects.inline_only_workflow_files, 1);
    assert_eq!(r.objects.inline_only_bytes, 7);
    assert_eq!(r.finish(), 1);
    assert!(!r.full_restore_verified);
}

#[tokio::test]
async fn deterministic_sample_and_byte_budgets_never_substitute_or_green() {
    let mut f = Fixture::new();
    f.object("objects/a", Some(b"abc"));
    f.object("objects/b", Some(b"def"));
    f.config.limits.hash_samples = 1;
    let mut a = f.report();
    a.database.records.get_mut("documents").unwrap().extend([
        reference(Uuid::from_u128(1), "objects/a", b"abc"),
        reference(Uuid::from_u128(2), "objects/b", b"def"),
    ]);
    let mut b = f.report();
    b.database
        .records
        .get_mut("documents")
        .unwrap()
        .extend(a.database.records["documents"].iter().rev().cloned());
    objects::scan(&f.config, &f.inventory, &mut a, &local_deadline())
        .await
        .unwrap();
    objects::scan(&f.config, &f.inventory, &mut b, &local_deadline())
        .await
        .unwrap();
    assert_eq!(a.objects.hash_selected, b.objects.hash_selected);
    assert_eq!(a.objects.hash_checked, 1);
    assert_eq!(a.objects.hash_eligible, 2);
    assert_eq!(a.finish(), 0);
    assert_eq!(a.verdict, "no_anomalies_in_completed_checks");
    assert!(!a.full_restore_verified);
    f.config.limits.hash_object_bytes = 2;
    let error = objects::scan(&f.config, &f.inventory, &mut b, &local_deadline())
        .await
        .unwrap_err();
    b.failures.push(error);
    assert_eq!(b.finish(), 2);
    f.config.limits.hash_object_bytes = 1024;
    f.config.limits.hash_total_bytes = 2;
    let mut c = f.report();
    c.database.records = a.database.records;
    assert_eq!(
        objects::scan(&f.config, &f.inventory, &mut c, &local_deadline()).await,
        Err("hash_byte_budget_exceeded")
    );
    assert_eq!(c.finish(), 2);
}

#[tokio::test]
async fn incomplete_inventory_database_and_io_failures_never_green() {
    let mut f = Fixture::new();
    f.object("objects/a", Some(b"abc"));
    let mut r = f.report();
    r.database.scans.get_mut("documents").unwrap().complete = false;
    objects::scan(&f.config, &f.inventory, &mut r, &local_deadline())
        .await
        .unwrap();
    assert!(codes(&r).contains(&"unmatched_inventory_candidate"));
    assert!(!codes(&r).contains(&"inventory_orphan"));
    assert_eq!(r.finish(), 2);
    let mut r = f.report();
    r.inventory = Some(json!({"operator_attested_complete":false}));
    objects::scan(&f.config, &f.inventory, &mut r, &local_deadline())
        .await
        .unwrap();
    assert_eq!(r.finish(), 2);
    let mut r = f.report();
    r.database
        .records
        .get_mut("documents")
        .unwrap()
        .push(reference(Uuid::new_v4(), "objects/a", b"abc"));
    f.inventory.entries[0].byte_size = 2;
    assert_eq!(
        objects::scan(&f.config, &f.inventory, &mut r, &local_deadline()).await,
        Err("sample_size_changed_or_inventory_inconsistent")
    );
    assert!(codes(&r).contains(&"inventory_size_mismatch"));
    assert_eq!(r.finish(), 2);
}

#[tokio::test]
async fn postgres_current_schema_evidence_snapshot_and_read_only() {
    // The shared fixture creates a private clone on the explicitly configured
    // disposable server. CI fails rather than skipping when DB is required.
    if let (Ok(db), Ok(test)) = (
        std::env::var("DATABASE_URL"),
        std::env::var("TEST_DATABASE_URL"),
    ) {
        assert_eq!(db, test, "disposable URLs must match");
    }
    let Some(url) = deepref_app::test_isolation::isolated_url_from_env("DATABASE_URL").await else {
        return;
    };
    let pool = PgPool::connect(&url).await.unwrap();
    let mut f = Fixture::new();
    seed_database(&pool, &mut f).await;
    let before: Value = sqlx::query_scalar(
        "SELECT jsonb_agg(to_jsonb(r) ORDER BY r.id) FROM ai_usage_reservations r",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut r = f.report();
    db::scan(&f.config, &url, &mut r.database, &local_deadline())
        .await
        .unwrap();
    assert_eq!(r.database.snapshot.as_ref().unwrap()["read_only"], "on");
    assert_eq!(
        r.database.snapshot.as_ref().unwrap()["isolation"],
        "repeatable read"
    );
    assert!(r.database.scans.values().all(|s| s.complete));
    let jobs = &r.database.records["jobs"];
    assert!(
        jobs.iter()
            .any(|v| v["recovery_evidence"] == "expired_retry_candidate")
    );
    assert!(
        jobs.iter()
            .any(|v| v["recovery_evidence"] == "expired_dead_letter_candidate")
    );
    assert!(
        jobs.iter()
            .any(|v| v["recovery_evidence"] == "unknown_kind_requires_inspection")
    );
    assert!(
        jobs.iter().any(|v| v["lease_expired_at_snapshot"] == true
            && v["lease_expired_at_restore_target"] == false)
    );
    let future_ownerless = jobs
        .iter()
        .find(|v| v["recovery_evidence"] == "lease_not_expired" && v["stale"] == false)
        .unwrap();
    assert_eq!(future_ownerless["lease_owner_present"], false);
    assert!(
        db::operational_findings(&r.database, &local_deadline())
            .unwrap()
            .iter()
            .any(|v| v["evidence"]["id"] == future_ownerless["id"])
    );
    assert!(
        jobs.iter()
            .any(|v| v["recovery_evidence"] == "expired_retry_candidate"
                && v["lease_owner_present"] == false)
    );
    let reservations = &r.database.records["reservations"];
    for fence in [
        "job_missing",
        "superseded_attempt",
        "job_released",
        "not_fenced_for_reconciliation",
        "request_owned_no_job_fence",
    ] {
        assert!(
            reservations.iter().any(|v| v["recovery_fence"] == fence),
            "{fence}"
        );
    }
    assert!(
        reservations
            .iter()
            .any(|v| v["observation_complete"] == true)
    );
    assert!(
        reservations
            .iter()
            .any(|v| v["observation_complete"] == false)
    );
    r.findings
        .extend(db::operational_findings(&r.database, &local_deadline()).unwrap());
    objects::scan(&f.config, &f.inventory, &mut r, &local_deadline())
        .await
        .unwrap();
    assert_eq!(r.objects.inline_only_workflow_files, 1);
    assert_eq!(r.objects.documents_already_marked_blob_missing, 1);
    assert_eq!(r.objects.documents_without_object, 1);
    assert!(codes(&r).contains(&"invalid_integrity_metadata"));
    assert!(codes(&r).contains(&"referenced_materialization_missing"));
    assert!(codes(&r).contains(&"inbox_requires_review"));
    assert!(codes(&r).contains(&"outbox_requires_review"));
    assert!(codes(&r).contains(&"reservations_requires_review"));
    assert_eq!(r.finish(), 1);
    let after: Value = sqlx::query_scalar(
        "SELECT jsonb_agg(to_jsonb(r) ORDER BY r.id) FROM ai_usage_reservations r",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before, after);
    // Same connection constructor and transaction contract as the real command.
    let mut conn = db::connect(&url, 5000).await.unwrap();
    let mut tx = conn
        .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM integration_inbox_events")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO integration_inbox_events (provider,provider_event_id,event_type,payload) VALUES ('fixture','new','event','{}')").execute(&pool).await.unwrap();
    let same: i64 = sqlx::query_scalar("SELECT count(*) FROM integration_inbox_events")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(n, same);
    let err = sqlx::query("DELETE FROM integration_inbox_events")
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert_eq!(
        err.as_database_error().unwrap().code().as_deref(),
        Some("25006")
    );
    tx.rollback().await.unwrap();
    f.config.limits.rows_per_table = 1;
    let mut r = f.report();
    db::scan(&f.config, &url, &mut r.database, &local_deadline())
        .await
        .unwrap();
    assert!(!r.database.scans["jobs"].complete);
    assert_eq!(r.finish(), 2);
    // Unavailable current schema produces sanitized failure and incomplete evidence.
    sqlx::query("ALTER TABLE documents RENAME COLUMN object_key TO fixture_old_key")
        .execute(&pool)
        .await
        .unwrap();
    let mut r = f.report();
    assert_eq!(
        db::scan(&f.config, &url, &mut r.database, &local_deadline()).await,
        Err("database_scan_failed")
    );
    assert_eq!(r.finish(), 2);
    let mut connection = db::connect(&url, 10).await.unwrap();
    let err = sqlx::query("SELECT pg_sleep(0.1)")
        .execute(&mut connection)
        .await
        .unwrap_err();
    assert_eq!(
        err.as_database_error().unwrap().code().as_deref(),
        Some("57014")
    );
    pool.close().await;
}

async fn seed_database(pool: &PgPool, f: &mut Fixture) {
    let project = Uuid::new_v4();
    let run = Uuid::new_v4();
    let report = Uuid::new_v4();
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'restore fixture')")
        .bind(project)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO reports(id,title) VALUES($1,'restore fixture')")
        .bind(report)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO durable_runs(id,project_id,kind,actor_kind,actor_id,idempotency_key,status) VALUES($1,$2,'workflow','user','fixture',$1::text,'queued')").bind(run).bind(project).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO workflow_files(project_id,run_id,node_id,name,content_type,content) VALUES($1,$2,'export','inline','text/plain',$3)").bind(project).bind(run).bind(b"inline".as_slice()).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO project_reports(project_id,report_id) VALUES($1,$2)")
        .bind(project)
        .bind(report)
        .execute(pool)
        .await
        .unwrap();
    f.object(
        "documents/00000000-0000-0000-0000-000000000001",
        Some(b"abc"),
    );
    sqlx::query("INSERT INTO documents(id,report_id,object_key,content_hash,mime_type,byte_size,status,project_id) VALUES($1,$2,'documents/00000000-0000-0000-0000-000000000001',$3,'application/pdf',3,'uploaded',$4)").bind(Uuid::new_v4()).bind(report).bind(objects::digest(b"abc")).bind(project).execute(pool).await.unwrap();
    let failed_key = "documents/00000000-0000-0000-0000-000000000002";
    f.object(failed_key, None);
    sqlx::query("INSERT INTO documents(id,report_id,project_id,object_key,content_hash,mime_type,byte_size,status,parser_error) VALUES($1,$2,$3,$4,$5,'application/pdf',3,'failed','document_blob_missing')").bind(Uuid::new_v4()).bind(report).bind(project).bind(failed_key).bind(objects::digest(b"def")).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO documents(id,report_id,project_id,byte_size,status,mime_type) VALUES($1,$2,$3,0,'missing','application/pdf')").bind(Uuid::new_v4()).bind(report).bind(project).execute(pool).await.unwrap();
    // The released validated CHECK accepts this partial-null shape through
    // SQL NULL semantics. Reconciliation must reject it as metadata evidence.
    let partial_id = Uuid::new_v4();
    let partial_key = format!("projects/{project}/workflow-runs/{run}/files/{partial_id}");
    f.object(&partial_key, Some(b"abc"));
    sqlx::query("INSERT INTO workflow_files(id,project_id,run_id,node_id,name,content_type,content,storage_key) VALUES($1,$2,$3,'export','partial','text/plain',$4,$5)").bind(partial_id).bind(project).bind(run).bind(b"abc".as_slice()).bind(&partial_key).execute(pool).await.unwrap();
    let backed_id = Uuid::new_v4();
    let backed_key = format!("projects/{project}/workflow-runs/{run}/files/{backed_id}");
    sqlx::query("INSERT INTO workflow_files(id,project_id,run_id,node_id,name,content_type,content,storage_key,content_sha256,byte_size) VALUES($1,$2,$3,'export','missing-object','text/plain',$4,$5,$6,3)").bind(backed_id).bind(project).bind(run).bind(b"abc".as_slice()).bind(&backed_key).bind(objects::digest(b"abc")).execute(pool).await.unwrap();
    let mut jobs = Vec::new();
    for (kind, state, attempts, claim_seq) in [
        ("recompute_metrics", "running", 1, 1),
        ("recompute_metrics", "running", 5, 1),
        ("future_unknown", "running", 1, 1),
        ("recompute_metrics", "completed", 1, 1),
        ("recompute_metrics", "running", 1, 2),
    ] {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO jobs(id,project_id,kind,state,attempts,claim_seq,lease_owner,leased_until,created_at) VALUES($1,$2,$3,$4,$5,$6,CASE WHEN $4='running' THEN 'fixture' END,CASE WHEN $4='running' THEN now()-interval '1 hour' END,now()-interval '2 hours')").bind(id).bind(project).bind(kind).bind(state).bind(attempts).bind(claim_seq as i64).execute(pool).await.unwrap();
        jobs.push(id);
    }
    for future in [false, true] {
        sqlx::query("INSERT INTO jobs(id,project_id,kind,state,attempts,claim_seq,lease_owner,leased_until,created_at) VALUES($1,$2,'recompute_metrics','running',1,1,NULL,now()+CASE WHEN $3 THEN interval '1 day' ELSE interval '-1 day' END,now())").bind(Uuid::new_v4()).bind(project).bind(future).execute(pool).await.unwrap();
    }
    sqlx::query("UPDATE jobs SET leased_until=$2 WHERE id=$1")
        .bind(jobs[0])
        .bind(f.config.restore_target)
        .execute(pool)
        .await
        .unwrap();
    for (job, observed) in [
        (Some(Uuid::new_v4()), false),
        (Some(jobs[4]), true),
        (Some(jobs[3]), true),
        (Some(jobs[0]), false),
        (None, false),
    ] {
        sqlx::query("INSERT INTO ai_usage_reservations(id,workspace_id,run_id,run_budget_id,job_id,attempt_fence,profile,provider,model,purpose,input_token_bound,max_output_tokens,input_micros_per_million_tokens,output_micros_per_million_tokens,price_version,request_fingerprint,reserved_cost_micros,period_start,state,observed_input_tokens,observed_output_tokens,opened_at) VALUES($1,$2,$3,$3,$4,CASE WHEN $4::uuid IS NOT NULL THEN 1 END,'fixture','fixture','fixture','structured',1,1,1,1,'fixture',repeat('a',64),1,current_date,CASE WHEN $5 THEN 'observed' ELSE 'open' END,CASE WHEN $5 THEN 1 END,CASE WHEN $5 THEN 1 END,now()-interval '2 hours')").bind(Uuid::new_v4()).bind(Uuid::new_v4()).bind(run).bind(job).bind(observed).execute(pool).await.unwrap();
    }
    sqlx::query("INSERT INTO integration_inbox_events(provider,provider_event_id,event_type,payload,received_at) VALUES('fixture','stale','event','{}',now()-interval '1 hour')").execute(pool).await.unwrap();
    sqlx::query("INSERT INTO outbox_events(event_id,message_type,message_version,payload,expected_consumers,created_at) VALUES($1,'fixture',1,'{}',ARRAY['fixture'],now()-interval '1 hour')").bind(Uuid::new_v4()).execute(pool).await.unwrap();
}

#[tokio::test]
async fn zero_hash_sample_still_checks_all_sizes_and_nonregular_objects() {
    let mut f = Fixture::new();
    f.config.limits.hash_samples = 0;
    f.object("objects/a", Some(b"abc"));
    let mut r = f.report();
    r.database
        .records
        .get_mut("documents")
        .unwrap()
        .push(reference(Uuid::new_v4(), "objects/a", b"abcdef"));
    objects::scan(&f.config, &f.inventory, &mut r, &local_deadline())
        .await
        .unwrap();
    assert_eq!(r.objects.hash_checked, 0);
    assert_eq!(r.objects.references_statted, 1);
    assert!(codes(&r).contains(&"metadata_materialization_size_mismatch"));
    assert_eq!(r.finish(), 1);
    std::fs::remove_file(f.root.join("objects/a")).unwrap();
    std::fs::create_dir(f.root.join("objects/a")).unwrap();
    let mut r = f.report();
    assert_eq!(
        objects::scan(&f.config, &f.inventory, &mut r, &local_deadline()).await,
        Err("nonregular_materialization_refused")
    );
    assert_eq!(r.finish(), 2);
}

#[tokio::test]
async fn cooperative_deadline_stops_local_scan_after_clock_expires() {
    use std::{
        cell::Cell,
        rc::Rc,
        time::{Duration, Instant},
    };
    let mut f = Fixture::new();
    f.config.limits.hash_samples = 0;
    for key in ["objects/a", "objects/b", "objects/c"] {
        f.object(key, Some(b"abc"));
    }
    let ticks = Rc::new(Cell::new(0_u32));
    let observed = Rc::clone(&ticks);
    let start = Instant::now();
    let deadline =
        super::deadline::Deadline::with_clock(start + Duration::from_millis(30), move || {
            let tick = observed.get();
            observed.set(tick + 1);
            start + Duration::from_millis(u64::from(tick))
        });
    let mut r = f.report();
    let result = objects::scan(&f.config, &f.inventory, &mut r, &deadline).await;
    assert_eq!(result, Err("overall_timeout"));
    assert!(
        ticks.get() > 1,
        "clock advanced during bounded local operations"
    );
    assert!(r.objects.inventory_checked > 0);
    assert!(r.objects.inventory_checked < f.inventory.entries.len());
    assert!(!r.objects.materializations_complete);
    r.failures.push(result.unwrap_err());
    assert_eq!(r.finish(), 2);
}
