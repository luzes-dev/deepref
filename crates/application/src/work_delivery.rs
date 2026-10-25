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
/// Closed diagnostics: Serde's Display/source can contain private scalar values
/// and field names. Discard it at the wire boundary, retaining only location.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("wire_{category} at {line}:{column}")]
pub struct WireError {
    pub category: &'static str,
    pub line: usize,
    pub column: usize,
}
impl WireError {
    fn json(error: serde_json::Error) -> Self {
        Self {
            category: match error.classify() {
                serde_json::error::Category::Io => "io",
                serde_json::error::Category::Syntax => "syntax",
                serde_json::error::Category::Data => "structure",
                serde_json::error::Category::Eof => "incomplete",
            },
            line: error.line(),
            column: error.column(),
        }
    }
    fn policy() -> Self {
        Self {
            category: "policy",
            line: 0,
            column: 0,
        }
    }
}
impl EventEnvelope {
    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let envelope: Self = serde_json::from_slice(bytes).map_err(WireError::json)?;
        if envelope.envelope_version != 1
            || envelope.message_version == 0
            || envelope.publication_epoch < 0
            || envelope.message_type.is_empty()
        {
            return Err(WireError::policy());
        }
        Ok(envelope)
    }

    pub fn work(&self) -> Result<WorkWakeup, WireError> {
        if self.envelope_version != 1
            || self.message_type != WorkWakeup::TYPE
            || self.message_version != WorkWakeup::VERSION
        {
            return Err(WireError::policy());
        }
        let wakeup: WorkWakeup =
            serde_json::from_value(self.payload.clone()).map_err(WireError::json)?;
        if wakeup.generation <= 0 {
            return Err(WireError::policy());
        }
        Ok(wakeup)
    }
}
