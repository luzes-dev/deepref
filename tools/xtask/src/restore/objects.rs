use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use futures::StreamExt;
use object_store::{ObjectStore, ObjectStoreExt, local::LocalFileSystem, path::Path};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{
    CheckResult, Report,
    config::{Config, reject_symlinks, validate_key},
    inventory::Inventory,
};

#[derive(Debug, Deserialize)]
struct Reference {
    id: String,
    key: Option<String>,
    sha256: Option<String>,
    byte_size: Option<u64>,
    inline_bytes: Option<u64>,
    status: Option<String>,
    expected_key: Option<String>,
    #[serde(default)]
    already_marked_blob_missing: Option<bool>,
}

pub(super) async fn scan(
    config: &Config,
    inventory: &Inventory,
    report: &mut Report,
    deadline: &super::deadline::Deadline,
) -> CheckResult<()> {
    deadline.check()?;
    // Unlike DocumentStore::from_config/local, this does not create a root.
    // Use the repository's object_store boundary directly; no application boot.
    let store: Arc<dyn ObjectStore> = Arc::new(
        LocalFileSystem::new_with_prefix(&config.quarantine_root)
            .map_err(|_| "quarantine_store_failed")?,
    );
    deadline.check()?;
    let mut entries = BTreeMap::new();
    for entry in &inventory.entries {
        deadline.check()?;
        entries.insert(entry.key.as_str(), entry);
    }
    let mut present = BTreeSet::new();
    for (key, entry) in &entries {
        deadline.check()?;
        match materialized_size(config, key, deadline)? {
            Some(size) => {
                present.insert(*key);
                if size != entry.byte_size {
                    report.findings.push(json!({"code":"inventory_size_mismatch","key":key,
                        "inventory_bytes":entry.byte_size,"materialized_bytes":size}));
                }
            }
            None => report.findings.push(json!({
                "code": if entry.version_id.is_some() { "selected_version_materialization_missing" } else { "materialization_missing" },
                "key": key,"inventory_version_id":entry.version_id,
            })),
        }
        report.objects.inventory_checked += 1;
    }
    deadline.check()?;
    let mut referenced = BTreeSet::new();
    let mut samples = Vec::new();
    for table in ["documents", "workflow_files"] {
        for raw in report.database.records.get(table).into_iter().flatten() {
            deadline.check()?;
            let reference: Reference =
                serde_json::from_value(raw.clone()).map_err(|_| "metadata_decode_failed")?;
            let Some(key) = &reference.key else {
                if table == "workflow_files"
                    && reference.inline_bytes.is_some()
                    && reference.sha256.is_none()
                    && reference.byte_size.is_none()
                {
                    report.objects.inline_only_workflow_files += 1;
                    report.objects.inline_only_bytes += reference.inline_bytes.unwrap_or(0);
                    continue;
                }
                if table == "documents"
                    && reference.sha256.is_none()
                    && reference.byte_size == Some(0)
                    && matches!(
                        reference.status.as_deref(),
                        Some("missing" | "external" | "retrieving" | "failed")
                    )
                {
                    report.objects.documents_without_object += 1;
                    continue;
                }
                report.findings.push(
                    json!({"code":"invalid_integrity_metadata","source":table,"id":reference.id}),
                );
                continue;
            };
            validate_key(key)?;
            let actual_size = materialized_size(config, key, deadline)?;
            report.objects.references_statted += 1;
            if reference.already_marked_blob_missing == Some(true) {
                report.objects.documents_already_marked_blob_missing += 1;
            }
            if actual_size.is_none() {
                report.findings.push(json!({"code":"referenced_materialization_missing","source":table,"id":reference.id,"key":key}));
            } else if reference.byte_size != actual_size {
                report.findings.push(json!({"code":"metadata_materialization_size_mismatch","source":table,"id":reference.id,"key":key}));
            }
            if reference
                .expected_key
                .as_ref()
                .is_some_and(|expected| expected != key)
                || (table == "workflow_files"
                    && reference.inline_bytes.is_some()
                    && reference.inline_bytes != reference.byte_size)
                || (table == "documents" && reference.byte_size == Some(0))
            {
                report.findings.push(
                    json!({"code":"invalid_integrity_metadata","source":table,"id":reference.id}),
                );
            }
            referenced.insert(key.clone());
            report.objects.references += 1;
            let valid_hash = reference.sha256.as_ref().is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            });
            if !valid_hash || reference.byte_size.is_none() {
                report.findings.push(
                    json!({"code":"invalid_integrity_metadata","source":table,"id":reference.id}),
                );
                continue;
            }
            let Some(entry) = entries.get(key.as_str()) else {
                report.findings.push(json!({"code":"reference_absent_from_inventory","source":table,"id":reference.id,"key":key}));
                continue;
            };
            if reference.byte_size != Some(entry.byte_size) {
                report.findings.push(json!({"code":"metadata_inventory_size_mismatch","source":table,"id":reference.id,"key":key}));
            }
            if present.contains(key.as_str()) {
                let rank = digest(format!("{table}\0{}\0{key}", reference.id).as_bytes());
                samples.push((rank, table, reference));
            }
        }
    }
    deadline.check()?;
    report.objects.materializations_complete = true;
    let metadata_complete = ["documents", "workflow_files"].iter().all(|table| {
        report
            .database
            .scans
            .get(*table)
            .is_some_and(|s| s.complete)
    });
    for key in entries.keys() {
        deadline.check()?;
        if !referenced.contains(*key) {
            report.findings.push(json!({"code":if metadata_complete {"inventory_orphan"} else {"unmatched_inventory_candidate"},"key":key}));
        }
    }
    report.objects.orphan_comparison_complete = metadata_complete;
    deadline.check()?;
    samples.sort_by(|a, b| (&a.0, a.1, &a.2.id).cmp(&(&b.0, b.1, &b.2.id)));
    deadline.check()?;
    report.objects.hash_eligible = samples.len();
    for (_, table, reference) in samples.into_iter().take(config.limits.hash_samples) {
        deadline.check()?;
        let key = reference
            .key
            .as_deref()
            .ok_or("internal_reference_missing")?;
        report
            .objects
            .hash_selected
            .push(json!({"source":table,"id":reference.id,"key":key}));
        let size = entries[key].byte_size;
        if size > config.limits.hash_object_bytes
            || size
                > config
                    .limits
                    .hash_total_bytes
                    .saturating_sub(report.objects.hash_bytes)
        {
            return Err("hash_byte_budget_exceeded");
        }
        reject_symlinks(&config.quarantine_root.join(key), Some(deadline))?;
        let result = store
            .get(&Path::from(key))
            .await
            .map_err(|_| "object_get_failed")?;
        if result.meta.size != size {
            // Manifest and store differ: cannot substitute a different-size sample.
            return Err("sample_size_changed_or_inventory_inconsistent");
        }
        let mut stream = result.into_stream();
        let mut hasher = Sha256::new();
        let mut bytes = 0;
        while let Some(chunk) = stream.next().await {
            deadline.check()?;
            let chunk = chunk.map_err(|_| "object_stream_failed")?;
            bytes += chunk.len() as u64;
            if bytes > size
                || bytes > config.limits.hash_object_bytes
                || bytes
                    > config
                        .limits
                        .hash_total_bytes
                        .saturating_sub(report.objects.hash_bytes)
            {
                return Err("hash_byte_budget_exceeded_or_object_changed");
            }
            hasher.update(&chunk);
        }
        report.objects.hash_bytes += bytes;
        if bytes != size {
            return Err("sample_truncated_or_object_changed");
        }
        let actual = hex(&hasher.finalize());
        if reference.sha256.as_deref() != Some(&actual) {
            report.findings.push(
                json!({"code":"sampled_hash_mismatch","source":table,"id":reference.id,"key":key}),
            );
        }
        report.objects.hash_checked += 1;
    }
    deadline.check()?;
    report.objects.hash_scan_complete = true;
    Ok(())
}

pub(super) fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn materialized_size(
    config: &Config,
    key: &str,
    deadline: &super::deadline::Deadline,
) -> CheckResult<Option<u64>> {
    deadline.check()?;
    let path = config.quarantine_root.join(key);
    reject_symlinks(&path, Some(deadline))?;
    let metadata = std::fs::symlink_metadata(path);
    deadline.check()?;
    match metadata {
        Ok(meta) if meta.is_file() => Ok(Some(meta.len())),
        Ok(_) => Err("nonregular_materialization_refused"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("materialization_stat_failed"),
    }
}
