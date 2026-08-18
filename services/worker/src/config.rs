use std::time::Duration;

use deepref_config::RuntimeConfig;

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub runtime: RuntimeConfig,
    pub concurrency: usize,
    pub claim_lease: Duration,
    pub reconciler_interval: Duration,
}

impl WorkerConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let concurrency = parse("WORKER_CONCURRENCY", 8_usize)?;
        if concurrency == 0 {
            anyhow::bail!("WORKER_CONCURRENCY must be >= 1");
        }
        let lease = parse("WORKER_CLAIM_LEASE_SECS", 60_u64)?;
        let reconcile = parse("WORKER_RECONCILER_INTERVAL_SECS", 30_u64)?;
        if lease == 0 || reconcile == 0 {
            anyhow::bail!("worker durations must be greater than zero");
        }
        Ok(Self {
            runtime: RuntimeConfig::from_env("deepref-worker")?,
            concurrency,
            claim_lease: Duration::from_secs(lease),
            reconciler_interval: Duration::from_secs(reconcile),
        })
    }
}

fn parse<T>(name: &str, default: T) -> anyhow::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    Ok(std::env::var(name)
        .ok()
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(default))
}
