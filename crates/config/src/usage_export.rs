use crate::{ConfigError, Environment};

/// Turns the provider-neutral AI usage fact export on. Off by default: while it
/// is off, the replay emits no outbox events and the worker registers no
/// export consumer, so retained usage facts wait in the ledger and a later
/// replay emits every one of them.
pub const USAGE_EXPORT_ENABLED: &str = "DEEPREF_USAGE_EXPORT_ENABLED";

/// Usage facts one replay batch emits at most. The replay is bounded and
/// resumable; a backlog drains over as many batches as it needs.
pub const USAGE_EXPORT_REPLAY_BATCH_SIZE: &str = "DEEPREF_USAGE_EXPORT_REPLAY_BATCH_SIZE";

const DEFAULT_REPLAY_BATCH_SIZE: i64 = 500;
const MIN_REPLAY_BATCH_SIZE: i64 = 1;
const MAX_REPLAY_BATCH_SIZE: i64 = 1000;

/// Provider-neutral usage export settings (handoff 09 slice B). No billing
/// vendor appears here: the export sink is a port, and its adapter is chosen
/// with handoff 08.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageExportConfig {
    pub enabled: bool,
    pub replay_batch_size: i64,
}

impl Default for UsageExportConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            replay_batch_size: DEFAULT_REPLAY_BATCH_SIZE,
        }
    }
}

impl UsageExportConfig {
    pub(crate) fn from_source(
        _environment: Environment,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let enabled = crate::flag(get, USAGE_EXPORT_ENABLED)?;
        let replay_batch_size: i64 = crate::parse(
            get,
            USAGE_EXPORT_REPLAY_BATCH_SIZE,
            DEFAULT_REPLAY_BATCH_SIZE,
        )?;
        if !(MIN_REPLAY_BATCH_SIZE..=MAX_REPLAY_BATCH_SIZE).contains(&replay_batch_size) {
            return Err(ConfigError::Invalid {
                name: USAGE_EXPORT_REPLAY_BATCH_SIZE,
                reason: format!(
                    "must be between {MIN_REPLAY_BATCH_SIZE} and {MAX_REPLAY_BATCH_SIZE}"
                ),
            });
        }
        Ok(Self {
            enabled,
            replay_batch_size,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn load(values: &[(&str, &str)]) -> Result<UsageExportConfig, ConfigError> {
        let values: HashMap<String, String> = values
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        UsageExportConfig::from_source(Environment::Development, &|name| values.get(name).cloned())
    }

    #[test]
    fn export_is_disabled_by_default() {
        let config = load(&[]).expect("defaults load");
        assert!(!config.enabled);
        assert_eq!(config.replay_batch_size, 500);
    }

    #[test]
    fn the_flag_accepts_the_documented_spellings() {
        for value in ["1", "true", "TRUE", "yes"] {
            assert!(
                load(&[(USAGE_EXPORT_ENABLED, value)])
                    .expect("flag loads")
                    .enabled
            );
        }
        for value in ["0", "false", "no"] {
            assert!(
                !load(&[(USAGE_EXPORT_ENABLED, value)])
                    .expect("flag loads")
                    .enabled
            );
        }
        let error = load(&[(USAGE_EXPORT_ENABLED, "on")]).expect_err("unknown spelling is invalid");
        assert!(error.to_string().contains(USAGE_EXPORT_ENABLED));
    }

    #[test]
    fn the_replay_batch_size_is_bounded() {
        assert_eq!(
            load(&[(USAGE_EXPORT_REPLAY_BATCH_SIZE, "1")])
                .expect("batch loads")
                .replay_batch_size,
            1
        );
        assert_eq!(
            load(&[(USAGE_EXPORT_REPLAY_BATCH_SIZE, "1000")])
                .expect("batch loads")
                .replay_batch_size,
            1000
        );
        for value in ["0", "1001", "-5", "many"] {
            let error = load(&[(USAGE_EXPORT_REPLAY_BATCH_SIZE, value)])
                .expect_err("out-of-range batch is invalid");
            assert!(error.to_string().contains(USAGE_EXPORT_REPLAY_BATCH_SIZE));
        }
    }
}
