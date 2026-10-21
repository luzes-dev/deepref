use std::{collections::HashMap, time::Duration};

use deepref_domain::WorkloadClass;

use crate::{ConfigError, optional, parse};

#[derive(Debug, Clone)]
pub struct WorkerSettings {
    /// Global worker concurrency: the default for every workload pool and the
    /// value a pool uses when it has no explicit setting.
    pub concurrency: usize,
    /// Concurrency per workload pool, so a saturated pool neither drains nor
    /// starves another. Each class defaults to `concurrency`.
    pub pool_concurrency: HashMap<WorkloadClass, usize>,
    /// Largest batch one claim statement may return, regardless of free pool
    /// capacity. Claiming is always bounded by `min(free capacity, this)`.
    pub claim_batch_max: usize,
    pub claim_lease: Duration,
    pub reconciler_interval: Duration,
    pub document_parse_concurrency: usize,
    pub grobid_url: Option<String>,
    pub grobid_timeout: Duration,
}

impl WorkerSettings {
    pub fn from_source(get: &dyn Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let concurrency = positive(get, "WORKER_CONCURRENCY", 8_usize)?;
        let claim_batch_max = positive(get, "WORKER_CLAIM_BATCH_MAX", 16_usize)?;
        let mut pool_concurrency = HashMap::new();
        for class in WorkloadClass::ALL {
            let name = pool_concurrency_variable(class);
            let value = parse(get, name, concurrency)?;
            if value == 0 {
                return Err(ConfigError::Invalid {
                    name,
                    reason: "must be greater than zero".into(),
                });
            }
            pool_concurrency.insert(class, value);
        }
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
        if grobid_url
            .as_ref()
            .is_some_and(|url| !(url.starts_with("http://") || url.starts_with("https://")))
        {
            return Err(ConfigError::Invalid {
                name: "GROBID_URL",
                reason: "expected an http(s) URL".into(),
            });
        }
        let grobid_timeout = get("GROBID_TIMEOUT_SECS")
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|secs| *secs > 0)
            .unwrap_or(120);
        Ok(Self {
            concurrency,
            pool_concurrency,
            claim_batch_max,
            claim_lease: Duration::from_secs(lease),
            reconciler_interval: Duration::from_secs(reconciler),
            document_parse_concurrency,
            grobid_url,
            grobid_timeout: Duration::from_secs(grobid_timeout),
        })
    }
}

/// The concurrency variable of one workload pool, spelled out so config errors
/// name a real environment variable.
fn pool_concurrency_variable(class: WorkloadClass) -> &'static str {
    match class {
        WorkloadClass::Ai => "WORKER_CONCURRENCY_AI",
        WorkloadClass::Documents => "WORKER_CONCURRENCY_DOCUMENTS",
        WorkloadClass::Workflow => "WORKER_CONCURRENCY_WORKFLOW",
        WorkloadClass::Integration => "WORKER_CONCURRENCY_INTEGRATION",
        WorkloadClass::Projection => "WORKER_CONCURRENCY_PROJECTION",
    }
}

fn positive<T>(
    get: &dyn Fn(&str) -> Option<String>,
    name: &'static str,
    default: T,
) -> Result<T, ConfigError>
where
    T: std::str::FromStr + Default + PartialEq,
    T::Err: std::fmt::Display,
{
    let value = parse(get, name, default)?;
    if value == T::default() {
        return Err(ConfigError::Invalid {
            name,
            reason: "must be greater than zero".into(),
        });
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
        assert_eq!(config.claim_batch_max, 16);
        assert_eq!(config.claim_lease, Duration::from_secs(60));
        assert_eq!(config.reconciler_interval, Duration::from_secs(30));
        assert_eq!(config.document_parse_concurrency, 2);
        assert_eq!(config.grobid_timeout, Duration::from_secs(120));
        // Every pool defaults to the global concurrency.
        for class in WorkloadClass::ALL {
            assert_eq!(config.pool_concurrency[&class], 8);
        }
        for value in ["0", "17", "invalid"] {
            assert!(
                WorkerSettings::from_source(
                    &|name| (name == "DOCUMENT_PARSE_CONCURRENCY").then(|| value.into())
                )
                .is_err()
            );
        }
        for name in [
            "WORKER_CONCURRENCY",
            "WORKER_CLAIM_BATCH_MAX",
            "WORKER_CLAIM_LEASE_SECS",
            "WORKER_RECONCILER_INTERVAL_SECS",
        ] {
            assert!(WorkerSettings::from_source(&|key| (key == name).then(|| "0".into())).is_err());
        }
    }

    /// Concurrency is set per pool; one pool's setting never leaks into
    /// another, and the unconfigured pools keep the global default.
    #[test]
    fn per_pool_concurrency_overrides_only_the_named_pool() {
        let config = WorkerSettings::from_source(&|name| match name {
            "WORKER_CONCURRENCY" => Some("8".into()),
            "WORKER_CONCURRENCY_AI" => Some("2".into()),
            "WORKER_CONCURRENCY_DOCUMENTS" => Some("4".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(config.pool_concurrency[&WorkloadClass::Ai], 2);
        assert_eq!(config.pool_concurrency[&WorkloadClass::Documents], 4);
        assert_eq!(config.pool_concurrency[&WorkloadClass::Workflow], 8);
        assert_eq!(config.pool_concurrency[&WorkloadClass::Integration], 8);
        assert_eq!(config.pool_concurrency[&WorkloadClass::Projection], 8);

        let error = WorkerSettings::from_source(&|name| {
            (name == "WORKER_CONCURRENCY_INTEGRATION").then(|| "0".into())
        })
        .expect_err("a pool must reject zero concurrency");
        assert!(
            matches!(&error, ConfigError::Invalid { name, .. } if *name == "WORKER_CONCURRENCY_INTEGRATION"),
            "{error}"
        );
    }
}
