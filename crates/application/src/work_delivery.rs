//! Deepref-owned wire contract. Infrastructure SDK objects never cross this port.
use chrono::{DateTime, Utc};
use deepref_domain::WorkloadClass;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::outbox::OutboxMessage;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkWakeup {
    pub job_id: Uuid,
    pub generation: i64,
    pub workload_class: WorkloadClass,
}
impl OutboxMessage for WorkWakeup {
    const TYPE: &'static str = "deepref.work";
    const VERSION: u32 = 1;
    const CONSUMERS: &'static [&'static str] = &["durable-work"];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventEnvelope {
    pub envelope_version: u32,
    pub event_id: Uuid,
    pub publication_epoch: i64,
    pub message_type: String,
    pub message_version: u32,
    pub occurred_at: DateTime<Utc>,
    pub trace_id: Option<String>,
    pub payload: serde_json::Value,
}
impl EventEnvelope {
    pub fn work(&self) -> anyhow::Result<WorkWakeup> {
        anyhow::ensure!(
            self.envelope_version == 1
                && self.message_type == WorkWakeup::TYPE
                && self.message_version == WorkWakeup::VERSION,
            "unsupported work envelope/version"
        );
        let wakeup: WorkWakeup = serde_json::from_value(self.payload.clone())?;
        anyhow::ensure!(wakeup.generation > 0, "invalid wakeup generation");
        Ok(wakeup)
    }
}
