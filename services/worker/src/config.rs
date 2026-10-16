use std::{sync::Arc, time::Duration};

use deepref_config::{DocumentConfig, RuntimeConfig, WorkerSettings};
use tokio::sync::Semaphore;

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub runtime: RuntimeConfig,
    pub documents: DocumentConfig,
    pub concurrency: usize,
    pub claim_lease: Duration,
    pub reconciler_interval: Duration,
    pub grobid_url: Option<String>,
    pub grobid_timeout: Duration,
    pub pdf_parse_semaphore: Arc<Semaphore>,
}

impl WorkerConfig {
    pub fn from_source(get: &dyn Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let runtime = RuntimeConfig::from_source("deepref-worker", get)?;
        let settings = WorkerSettings::from_source(get)?;
        Ok(Self {
            runtime,
            documents: DocumentConfig::from_source(get)?,
            concurrency: settings.concurrency,
            claim_lease: settings.claim_lease,
            reconciler_interval: settings.reconciler_interval,
            grobid_url: settings.grobid_url,
            grobid_timeout: settings.grobid_timeout,
            pdf_parse_semaphore: Arc::new(Semaphore::new(settings.document_parse_concurrency)),
        })
    }
}
