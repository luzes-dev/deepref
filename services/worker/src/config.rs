use std::{collections::HashMap, sync::Arc, time::Duration};

use deepref_config::{DocumentConfig, RuntimeConfig, WorkerSettings};
use deepref_domain::WorkloadClass;
use tokio::sync::Semaphore;

use crate::metadata::MetadataClients;

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub runtime: RuntimeConfig,
    pub documents: DocumentConfig,
    /// Default concurrency for pools without an explicit setting.
    pub concurrency: usize,
    /// Concurrency per workload pool.
    pub pool_concurrency: HashMap<WorkloadClass, usize>,
    /// Largest batch one claim statement may return.
    pub claim_batch_max: usize,
    pub claim_lease: Duration,
    pub reconciler_interval: Duration,
    pub grobid_url: Option<String>,
    pub grobid_timeout: Duration,
    pub pdf_parse_semaphore: Arc<Semaphore>,
    /// Built once per process at boot, so every job shares the OpenAlex key state and budget.
    pub metadata: MetadataClients,
}

impl WorkerConfig {
    pub fn from_source(get: &dyn Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let runtime = RuntimeConfig::from_source("deepref-worker", get)?;
        let settings = WorkerSettings::from_source(get)?;
        let metadata = MetadataClients::from_config(&runtime.integrations)?;
        Ok(Self {
            runtime,
            documents: DocumentConfig::from_source(get)?,
            concurrency: settings.concurrency,
            pool_concurrency: settings.pool_concurrency,
            claim_batch_max: settings.claim_batch_max,
            claim_lease: settings.claim_lease,
            reconciler_interval: settings.reconciler_interval,
            grobid_url: settings.grobid_url,
            grobid_timeout: settings.grobid_timeout,
            pdf_parse_semaphore: Arc::new(Semaphore::new(settings.document_parse_concurrency)),
            metadata,
        })
    }
}
