use std::{collections::BTreeMap, str::FromStr};

use chrono::{DateTime, Utc};
use deepref_domain::JobKind;
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, Row, postgres::PgConnectOptions};

use super::{CheckResult, config::Config};

#[derive(Debug, Default, Serialize)]
pub(super) struct Scan {
    pub rows: usize,
    pub complete: bool,
}

#[derive(Default, Serialize)]
pub(super) struct Database {
    pub snapshot: Option<Value>,
    pub scans: BTreeMap<String, Scan>,
    pub records: BTreeMap<String, Vec<Value>>,
}

pub(super) async fn connect(url: &str, timeout_ms: u64) -> CheckResult<PgConnection> {
    // Startup options make even the first transaction read-only. No dotenv,
    // app initialization, migrations, advisory locks or tenant settings.
    let options = PgConnectOptions::from_str(url)
        .map_err(|_| "database_url_invalid")?
        .options([
            ("default_transaction_read_only", "on".to_owned()),
            ("statement_timeout", timeout_ms.to_string()),
            ("lock_timeout", timeout_ms.min(1000).to_string()),
            (
                "idle_in_transaction_session_timeout",
                timeout_ms.to_string(),
            ),
        ]);
    PgConnection::connect_with(&options)
        .await
        .map_err(|_| "database_connect_failed")
}

pub(super) async fn scan(
    config: &Config,
    url: &str,
    report: &mut Database,
    deadline: &super::deadline::Deadline,
) -> CheckResult<()> {
    deadline.check()?;
    let mut connection = connect(url, config.limits.statement_timeout_ms).await?;
    deadline.check()?;
    let mut tx = connection
        .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .await
        .map_err(|_| "snapshot_begin_failed")?;
    let result = async {
        deadline.check()?;
        report.snapshot = Some(
            sqlx::query_scalar::<_, Value>(
                "SELECT jsonb_build_object('observed_at', transaction_timestamp(),
              'snapshot_id', pg_current_snapshot()::text,
              'read_only', current_setting('transaction_read_only'),
              'isolation', current_setting('transaction_isolation'))",
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| "snapshot_read_failed")?,
        );
        for table in [
            "documents",
            "workflow_files",
            "jobs",
            "inbox",
            "outbox",
            "reservations",
        ] {
            deadline.check()?;
            report.scans.insert(table.into(), Scan::default());
            report.records.insert(table.into(), Vec::new());
            let mut cursor: Option<String> = None;
            loop {
                deadline.check()?;
                let remaining = config.limits.rows_per_table - report.scans[table].rows;
                let limit = config.limits.page_rows.min(remaining + 1);
                let query = query(table);
                let mut query = sqlx::query(query).bind(&cursor).bind(limit as i64);
                if !matches!(table, "documents" | "workflow_files") {
                    query = query.bind(config.stale_before).bind(config.restore_target);
                }
                if table == "jobs" {
                    query = query.bind(JobKind::known_kinds());
                }
                let rows = query
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(|_| "database_scan_failed")?;
                deadline.check()?;
                let fetched = rows.len();
                let scan = report.scans.get_mut(table).ok_or("internal_scan_missing")?;
                let records = report
                    .records
                    .get_mut(table)
                    .ok_or("internal_scan_missing")?;
                for row in rows.into_iter().take(remaining) {
                    deadline.check()?;
                    cursor = Some(
                        row.try_get("cursor")
                            .map_err(|_| "database_decode_failed")?,
                    );
                    records.push(row.try_get("data").map_err(|_| "database_decode_failed")?);
                    scan.rows += 1;
                }
                if fetched > remaining {
                    break;
                }
                if fetched < limit {
                    scan.complete = true;
                    break;
                }
            }
        }
        deadline.check()
    }
    .await;
    tx.rollback()
        .await
        .map_err(|_| "snapshot_rollback_failed")?;
    result
}

fn query(table: &str) -> &'static str {
    match table {
        "documents" => "SELECT id::text AS cursor, jsonb_build_object('id',id,'key',object_key,
          'sha256',content_hash,'byte_size',byte_size,'status',status,
          'already_marked_blob_missing',parser_error='document_blob_missing') AS data
          FROM documents WHERE ($1::text IS NULL OR id > $1::uuid) ORDER BY id LIMIT $2",
        "workflow_files" => "SELECT id::text AS cursor, jsonb_build_object('id',id,'key',storage_key,
          'sha256',content_sha256,'byte_size',byte_size,'inline_bytes',octet_length((to_jsonb(workflow_files)->>'content')::bytea),
          'expected_key',CASE WHEN storage_key LIKE 'workspaces/%' THEN
            'workspaces/' || (SELECT workspace_id::text FROM projects WHERE projects.id=workflow_files.project_id)
            || '/projects/' || project_id::text || '/workflow-runs/' || run_id::text || '/files/'
            || coalesce(substring(storage_key from '/files/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$'),'<invalid>')
            ELSE 'projects/' || project_id::text || '/workflow-runs/' || run_id::text || '/files/' || coalesce(substring(storage_key from '/files/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$'),'<invalid>') END) AS data
          FROM workflow_files WHERE ($1::text IS NULL OR id > $1::uuid) ORDER BY id LIMIT $2",
        "jobs" => "SELECT id::text AS cursor, jsonb_build_object('id',id,'kind',kind,'state',state,
          'attempts',attempts,'max_attempts',max_attempts,'claim_seq',claim_seq,
          'lease_owner_present',lease_owner IS NOT NULL,
          'leased_until',leased_until,'lease_expired_at_restore_target',leased_until < $4,
          'lease_expired_at_snapshot',leased_until < transaction_timestamp(),'stale',created_at <= $3,'available_at',available_at,
          'recovery_evidence', CASE
            WHEN state <> 'running' THEN 'not_expired_running'
            WHEN leased_until IS NULL THEN 'running_without_lease'
            WHEN leased_until >= transaction_timestamp() THEN 'lease_not_expired'
            WHEN NOT (kind = ANY($5)) THEN 'unknown_kind_requires_inspection'
            WHEN attempts >= max_attempts THEN 'expired_dead_letter_candidate'
            ELSE 'expired_retry_candidate' END) AS data
          FROM jobs WHERE state IN ('queued','capability_wait','running')
          AND ($1::text IS NULL OR id > $1::uuid) ORDER BY id LIMIT $2",
        "inbox" => "SELECT id::text AS cursor, jsonb_build_object('id',id,'state',processing_state,
          'received_at',received_at,'stale',received_at <= $3,'attempts',attempts,
          'max_attempts',max_attempts,'due',next_attempt_at <= transaction_timestamp(),
          'lease_expires_at',lease_expires_at,
          'lease_expired_at_snapshot',lease_expires_at <= transaction_timestamp(),
          'lease_expired_at_restore_target',lease_expires_at <= $4) AS data
          FROM integration_inbox_events WHERE processing_state <> 'processed'
          AND ($1::text IS NULL OR id > $1::bigint) ORDER BY id LIMIT $2",
        "outbox" => "SELECT event_id::text AS cursor, jsonb_build_object('id',event_id,
          'state',delivery_state,'created_at',created_at,'stale',created_at <= $3,
          'attempts',attempts,'due',next_attempt_at <= transaction_timestamp(),
          'lease_expires_at',lease_expires_at,
          'lease_expired_at_snapshot',lease_expires_at <= transaction_timestamp(),
          'lease_expired_at_restore_target',lease_expires_at <= $4) AS data
          FROM outbox_events WHERE delivery_state <> 'delivered'
          AND ($1::text IS NULL OR event_id > $1::uuid) ORDER BY event_id LIMIT $2",
        "reservations" => "SELECT r.id::text AS cursor, jsonb_build_object('id',r.id,
          'state',r.state,'opened_at',r.opened_at,'stale',r.opened_at <= $3,
          'job_id',r.job_id,'attempt_fence',r.attempt_fence,'current_claim_seq',j.claim_seq,
          'job_state',j.state,'job_leased_until',j.leased_until,
          'job_lease_expired_at_snapshot',j.leased_until < transaction_timestamp(),
          'job_lease_expired_at_restore_target',j.leased_until < $4,
          'recognized_cost_micros',r.recognized_cost_micros,
          'observation_complete',r.observed_input_tokens IS NOT NULL
             AND r.observed_output_tokens IS NOT NULL
             AND (r.reasoning_micros_per_million_tokens IS NULL OR r.observed_reasoning_tokens IS NOT NULL),
          'recovery_fence',CASE
            WHEN r.job_id IS NULL THEN 'request_owned_no_job_fence'
            WHEN j.id IS NULL THEN 'job_missing'
            WHEN j.claim_seq <> r.attempt_fence AND (j.state <> 'running' OR j.lease_owner IS NOT NULL)
              THEN 'superseded_attempt'
            WHEN j.state <> 'running' AND j.lease_owner IS NULL AND j.leased_until IS NULL
              THEN 'job_released'
            ELSE 'not_fenced_for_reconciliation' END) AS data
          FROM ai_usage_reservations r LEFT JOIN jobs j ON j.id=r.job_id
          WHERE r.state IN ('open','observed','unresolved')
          AND ($1::text IS NULL OR r.id > $1::uuid) ORDER BY r.id LIMIT $2",
        _ => unreachable!("scan table is internal"),
    }
}

pub(super) fn operational_findings(
    database: &Database,
    deadline: &super::deadline::Deadline,
) -> CheckResult<Vec<Value>> {
    let mut findings = Vec::new();
    for (table, records) in &database.records {
        for record in records {
            deadline.check()?;
            let evidence = record["recovery_evidence"].as_str().unwrap_or("");
            let anomaly = match table.as_str() {
                "jobs" => {
                    (record["state"] == "running" && record["lease_owner_present"] == false)
                        || evidence.starts_with("expired_")
                        || matches!(
                            evidence,
                            "running_without_lease" | "unknown_kind_requires_inspection"
                        )
                        || record["stale"] == true
                }
                "inbox" | "outbox" => record["stale"] == true || record["state"] == "quarantined",
                "reservations" => record["stale"] == true,
                _ => false,
            };
            if anomaly {
                findings
                    .push(json!({"code":format!("{table}_requires_review"), "evidence":record}));
            }
        }
    }
    deadline.check()?;
    Ok(findings)
}

pub(super) fn observed_at(database: &Database) -> Option<DateTime<Utc>> {
    database
        .snapshot
        .as_ref()?
        .get("observed_at")?
        .as_str()?
        .parse()
        .ok()
}
