use std::{
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::CheckResult;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Config {
    pub database_url_env: String,
    pub quarantine_root: PathBuf,
    pub inventory: PathBuf,
    pub restore_target: DateTime<Utc>,
    pub stale_before: DateTime<Utc>,
    pub limits: Limits,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Limits {
    pub rows_per_table: usize,
    pub page_rows: usize,
    pub inventory_bytes: u64,
    pub inventory_rows: usize,
    pub hash_samples: usize,
    pub hash_object_bytes: u64,
    pub hash_total_bytes: u64,
    pub statement_timeout_ms: u64,
    pub overall_timeout_ms: u64,
}

impl Config {
    pub fn load(path: &Path) -> CheckResult<Self> {
        let config: Self = read_json(path, 64 * 1024)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> CheckResult<()> {
        let l = &self.limits;
        if self.database_url_env != "DEEPREF_RESTORE_DATABASE_URL"
            || !self.quarantine_root.is_absolute()
            || !self.inventory.is_absolute()
            || self.restore_target > Utc::now()
            || self.stale_before > Utc::now()
            || !(1..=100_000).contains(&l.rows_per_table)
            || !(1..=1000).contains(&l.page_rows)
            || !(1..=16 * 1024 * 1024).contains(&l.inventory_bytes)
            || !(1..=100_000).contains(&l.inventory_rows)
            || !(0..=1000).contains(&l.hash_samples)
            || !(1..=64 * 1024 * 1024).contains(&l.hash_object_bytes)
            || !(1..=1024 * 1024 * 1024).contains(&l.hash_total_bytes)
            || !(1..=60_000).contains(&l.statement_timeout_ms)
            || !(1..=600_000).contains(&l.overall_timeout_ms)
        {
            return Err("unsafe_or_invalid_config");
        }
        reject_symlinks(&self.quarantine_root)?;
        if !self.quarantine_root.is_dir() {
            return Err("quarantine_root_not_directory");
        }
        Ok(())
    }
}

pub(super) fn read_json<T: serde::de::DeserializeOwned>(path: &Path, limit: u64) -> CheckResult<T> {
    let metadata = std::fs::metadata(path).map_err(|_| "input_open_failed")?;
    if !metadata.is_file() {
        return Err("nonregular_input_refused");
    }
    if metadata.len() > limit {
        return Err("input_byte_limit");
    }
    let file = File::open(path).map_err(|_| "input_open_failed")?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "input_read_failed")?;
    if bytes.len() as u64 > limit {
        return Err("input_byte_limit");
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalid_input_json")
}

/// Validate before passing to object_store, which normalizes some path spellings.
pub(super) fn validate_key(key: &str) -> CheckResult<()> {
    if key.is_empty()
        || key.len() > 1024
        || key.contains(['\\', '%', ':'])
        || key.chars().any(char::is_control)
        || key
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("unsafe_object_key");
    }
    Ok(())
}

/// Refuse symlinks, even links pointing within the quarantine. The quarantine
/// must remain immutable: this is validation, not protection from a local writer
/// racing path lookup. Missing final materializations are handled by the scan.
pub(super) fn reject_symlinks(path: &Path) -> CheckResult<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        if !matches!(component, Component::RootDir | Component::Normal(_)) {
            return Err("unsafe_local_path");
        }
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => return Err("symlink_refused"),
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err("local_metadata_failed"),
        }
    }
    Ok(())
}
