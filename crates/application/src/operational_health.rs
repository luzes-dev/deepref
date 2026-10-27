//! Diagnostic observations only. These never grant execution or drain authority.
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkTransportMode {
    #[default]
    Postgres,
    JetStream,
}

impl WorkTransportMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::JetStream => "jetstream",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportReadiness {
    pub ready: bool,
    /// Time the adapter observed the result, never the time it was copied.
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerTransportEvidence {
    Postgres,
    JetStream(Option<TransportReadiness>),
}
