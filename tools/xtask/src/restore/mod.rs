//! Offline restore evidence only. No migrations, application startup, writes,
//! provider calls, historical version resolution or acceptance verdict.
mod config;
mod db;
mod inventory;
mod objects;
#[cfg(test)]
mod tests;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

use config::{Config, Limits};
use inventory::Inventory;

type CheckResult<T> = Result<T, &'static str>;

#[derive(Default, Serialize)]
struct ObjectChecks {
    inventory_checked: usize,
    materializations_complete: bool,
    references: usize,
    references_statted: usize,
    documents_without_object: usize,
    documents_already_marked_blob_missing: usize,
    inline_only_workflow_files: usize,
    inline_only_bytes: u64,
    orphan_comparison_complete: bool,
    hash_eligible: usize,
    hash_selected: Vec<Value>,
    hash_checked: usize,
    hash_bytes: u64,
    hash_scan_complete: bool,
}

#[derive(Serialize)]
struct Report {
    schema_version: u32,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    restore_target: Option<DateTime<Utc>>,
    stale_before: Option<DateTime<Utc>>,
    limits: Option<Limits>,
    inventory: Option<Value>,
    database: db::Database,
    objects: ObjectChecks,
    findings: Vec<Value>,
    finding_counts: std::collections::BTreeMap<String, usize>,
    failures: Vec<&'static str>,
    complete: bool,
    verdict: &'static str,
    full_restore_verified: bool,
    limitations: Vec<&'static str>,
}

impl Report {
    fn new() -> Self {
        Self {
            schema_version: 1,
            started_at: Utc::now(),
            finished_at: None,
            restore_target: None,
            stale_before: None,
            limits: None,
            inventory: None,
            database: db::Database::default(),
            objects: ObjectChecks::default(),
            findings: Vec::new(),
            finding_counts: std::collections::BTreeMap::new(),
            failures: Vec::new(),
            complete: false,
            verdict: "incomplete",
            full_restore_verified: false,
            limitations: vec![
                "inventory_complete_and_materialized_as_of_are_operator_attestations",
                "database_restore_target_is_operator_attested_not_verified_by_this_tool",
                "database_references_are_key_only_no_historical_version_agreement_verified",
                "inventory_versions_are_labels_no_cloud_pitr_or_version_resolution",
                "quarantine_must_remain_immutable_and_read_only_during_execution",
                "hashes_cover_only_the_reported_deterministic_sample",
                "inline_only_workflow_bytes_are_compatibility_content_not_object_migration_completion",
                "backlog_age_is_diagnostic_not_proof_of_unrecoverability_or_zero_cost",
                "reservation_recovery_fence_is_snapshot_evidence_not_authority_to_settle_or_void",
                "worker_recovery_egress_isolation_deletion_replay_rpo_rto_not_tested",
            ],
        }
    }

    fn finish(&mut self) -> u8 {
        self.finding_counts.clear();
        for finding in &self.findings {
            if let Some(code) = finding["code"].as_str() {
                *self.finding_counts.entry(code.to_owned()).or_default() += 1;
            }
        }
        self.complete = self.failures.is_empty()
            && self.database.scans.len() == 6
            && self.database.scans.values().all(|scan| scan.complete)
            && self
                .inventory
                .as_ref()
                .is_some_and(|v| v["operator_attested_complete"] == true)
            && self.objects.materializations_complete
            && self.objects.orphan_comparison_complete
            && self.objects.hash_scan_complete;
        self.finished_at = Some(Utc::now());
        if !self.complete {
            self.verdict = "incomplete";
            2
        } else if self.findings.is_empty() {
            self.verdict = "no_anomalies_in_completed_checks";
            0
        } else {
            self.verdict = "anomalies";
            1
        }
    }
}

pub fn run(args: &[String]) -> std::process::ExitCode {
    let mut report = Report::new();
    let result = match args {
        [flag, path] if flag == "--config" => Config::load(Path::new(path)).and_then(|config| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "runtime_initialization_failed")?
                .block_on(async {
                    let started = std::time::Instant::now();
                    let outcome = tokio::time::timeout(
                        Duration::from_millis(config.limits.overall_timeout_ms),
                        execute(&config, &mut report),
                    )
                    .await
                    .map_err(|_| "overall_timeout")?;
                    if started.elapsed() >= Duration::from_millis(config.limits.overall_timeout_ms)
                    {
                        return Err("overall_timeout");
                    }
                    outcome
                })
        }),
        _ => Err("usage_reconcile_storage_--config_path"),
    };
    if let Err(code) = result {
        report.failures.push(code);
    }
    let exit = report.finish();
    match serde_json::to_string_pretty(&report) {
        Ok(json) => println!("{json}"),
        Err(_) => {
            println!(
                "{{\"complete\":false,\"full_restore_verified\":false,\"verdict\":\"incomplete\",\"failures\":[\"report_encode_failed\"]}}"
            );
            return std::process::ExitCode::from(2);
        }
    }
    std::process::ExitCode::from(exit)
}

async fn execute(config: &Config, report: &mut Report) -> CheckResult<()> {
    report.restore_target = Some(config.restore_target);
    report.stale_before = Some(config.stale_before);
    report.limits = Some(config.limits.clone());
    let inventory = Inventory::load(config)?;
    report.inventory = Some(json!({
        "snapshot_id":inventory.snapshot_id,"captured_at":inventory.captured_at,
        "restore_target":inventory.restore_target,"selection":inventory.selection,
        "operator_attested_complete":inventory.complete,"rows":inventory.entries.len(),
        "version_labeled_rows":inventory.entries.iter().filter(|e| e.version_id.is_some()).count(),
        "external_snapshot_authenticity_verified":false,
        "database_reference_semantics":"key_only",
    }));
    if !inventory.complete {
        report.failures.push("inventory_declared_incomplete");
    }
    let url = std::env::var(&config.database_url_env)
        .map_err(|_| "explicit_drill_database_url_required")?;
    db::scan(config, &url, &mut report.database).await?;
    if db::observed_at(&report.database).is_none_or(|time| config.stale_before > time) {
        return Err("cutoff_after_database_observation");
    }
    report
        .findings
        .extend(db::operational_findings(&report.database));
    if report.database.scans.values().any(|scan| !scan.complete) {
        report.failures.push("database_row_budget_exceeded");
    }
    objects::scan(config, &inventory, report).await
}
