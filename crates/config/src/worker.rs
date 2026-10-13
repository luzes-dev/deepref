use std::time::Duration;

use crate::{ConfigError, optional, parse};

#[derive(Debug, Clone)]
pub struct WorkerSettings {
    pub concurrency: usize,
    pub claim_lease: Duration,
    pub reconciler_interval: Duration,
    pub document_parse_concurrency: usize,
    pub grobid_url: Option<String>,
    pub grobid_timeout: Duration,
}

impl WorkerSettings {
    pub fn from_source(get: &dyn Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let concurrency = positive(get, "WORKER_CONCURRENCY", 8_usize)?;
        let lease = positive(get, "WORKER_CLAIM_LEASE_SECS", 60_u64)?;
        let reconciler = positive(get, "WORKER_RECONCILER_INTERVAL_SECS", 30_u64)?;
        let document_parse_concurrency = positive(get, "DOCUMENT_PARSE_CONCURRENCY", 2_usize)?;
        if document_parse_concurrency > 16 {
            return Err(ConfigError::Invalid {
                name: "DOCUMENT_PARSE_CONCURRENCY",
                reason: "must be a positive integer up to 16".into(),
            });
        }
        let grobid_url = optional(get, "GROBID_URL");
        if grobid_url.as_ref().is_some_and(|url| !(url.starts_with("http://") || url.starts_with("https://"))) {
            return Err(ConfigError::Invalid {
                name: "GROBID_URL",
                reason: "expected an http(s) URL".into(),
            });
        }
        let grobid_timeout = get("GROBID_TIMEOUT_SECS")
            .and_then(|value| value.trim().parse::<u64>().ok()).filter(|secs| *secs > 0)
            .unwrap_or(120);
        Ok(Self {
            concurrency,
            claim_lease: Duration::from_secs(lease),
            reconciler_interval: Duration::from_secs(reconciler),
            document_parse_concurrency,
            grobid_url,
            grobid_timeout: Duration::from_secs(grobid_timeout),
        })
    }
}

fn positive<T>(get: &dyn Fn(&str) -> Option<String>, name: &'static str, default: T) -> Result<T, ConfigError>
where T: std::str::FromStr + Default + PartialEq, T::Err: std::fmt::Display {
    let value = parse(get, name, default)?;
    if value == T::default() {
        return Err(ConfigError::Invalid { name, reason: "must be greater than zero".into() });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_defaults_and_limits_use_injected_sources() {
        let config = WorkerSettings::from_source(&|_| None).unwrap();
        assert_eq!(config.concurrency, 8);
        assert_eq!(config.claim_lease, Duration::from_secs(60));
        assert_eq!(config.reconciler_interval, Duration::from_secs(30));
        assert_eq!(config.document_parse_concurrency, 2);
        assert_eq!(config.grobid_timeout, Duration::from_secs(120));
        for value in ["0", "17", "invalid"] {
            assert!(WorkerSettings::from_source(&|name| (name == "DOCUMENT_PARSE_CONCURRENCY").then(|| value.into())).is_err());
        }
        for name in ["WORKER_CONCURRENCY", "WORKER_CLAIM_LEASE_SECS", "WORKER_RECONCILER_INTERVAL_SECS"] {
            assert!(WorkerSettings::from_source(&|key| (key == name).then(|| "0".into())).is_err());
        }
    }
}
