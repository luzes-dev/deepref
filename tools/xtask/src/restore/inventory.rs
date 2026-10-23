use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{
    CheckResult,
    config::{Config, read_json, validate_key},
};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Inventory {
    pub schema_version: u32,
    pub snapshot_id: String,
    pub captured_at: DateTime<Utc>,
    pub restore_target: DateTime<Utc>,
    pub selection: String,
    pub complete: bool,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    pub key: String,
    pub byte_size: u64,
    pub version_id: Option<String>,
}

impl Inventory {
    pub fn load(config: &Config, deadline: &super::deadline::Deadline) -> CheckResult<Self> {
        deadline.check()?;
        let inventory: Self = read_json(
            &config.inventory,
            config.limits.inventory_bytes,
            Some(deadline),
        )?;
        inventory.validate(config, deadline)?;
        Ok(inventory)
    }

    pub fn validate(
        &self,
        config: &Config,
        deadline: &super::deadline::Deadline,
    ) -> CheckResult<()> {
        deadline.check()?;
        if self.schema_version != 1
            || self.selection != "materialized_as_of"
            || self.restore_target != config.restore_target
            || self.captured_at < self.restore_target
            || self.captured_at > Utc::now()
            || self.snapshot_id.is_empty()
            || self.snapshot_id.len() > 200
            || self.snapshot_id.chars().any(char::is_control)
            || self.entries.len() > config.limits.inventory_rows
        {
            return Err("inventory_contract_invalid_or_limit");
        }
        let mut seen = BTreeMap::new();
        for entry in &self.entries {
            deadline.check()?;
            validate_key(&entry.key)?;
            if entry
                .version_id
                .as_ref()
                .is_some_and(|v| v.is_empty() || v.len() > 1024 || v.chars().any(char::is_control))
            {
                return Err("inventory_version_invalid");
            }
            if seen.insert(&entry.key, ()).is_some() {
                return Err("inventory_duplicate_key");
            }
        }
        deadline.check()
    }
}
