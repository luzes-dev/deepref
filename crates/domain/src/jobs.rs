use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Durable queue kinds. Spellings are the persisted database and JSON contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    WorkFetchRequested,
    RecomputeMetrics,
    #[serde(rename = "run_deduplication")]
    ImportDeduplication,
    PmidImport,
    AutomationRun,
    AssistantAgentRun,
    WorkflowStep,
    RecomputePrisma,
    RetrieveDocument,
    ParseDocument,
}

impl JobKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorkFetchRequested => "work_fetch_requested",
            Self::RecomputeMetrics => "recompute_metrics",
            Self::ImportDeduplication => "run_deduplication",
            Self::PmidImport => "pmid_import",
            Self::AutomationRun => "automation_run",
            Self::AssistantAgentRun => "assistant_agent_run",
            Self::WorkflowStep => "workflow_step",
            Self::RecomputePrisma => "recompute_prisma",
            Self::RetrieveDocument => "retrieve_document",
            Self::ParseDocument => "parse_document",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown durable job kind: {0}")]
pub struct InvalidJobKind(pub String);

impl std::str::FromStr for JobKind {
    type Err = InvalidJobKind;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "work_fetch_requested" => Ok(Self::WorkFetchRequested),
            "recompute_metrics" => Ok(Self::RecomputeMetrics),
            "run_deduplication" => Ok(Self::ImportDeduplication),
            "pmid_import" => Ok(Self::PmidImport),
            "automation_run" => Ok(Self::AutomationRun),
            "assistant_agent_run" => Ok(Self::AssistantAgentRun),
            "workflow_step" => Ok(Self::WorkflowStep),
            "recompute_prisma" => Ok(Self::RecomputePrisma),
            "retrieve_document" => Ok(Self::RetrieveDocument),
            "parse_document" => Ok(Self::ParseDocument),
            _ => Err(InvalidJobKind(value.to_owned())),
        }
    }
}

impl std::fmt::Display for JobKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Completed,
    Failed,
    Dead,
}

impl JobState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Dead => "dead",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown durable job state: {0}")]
pub struct InvalidJobState(pub String);

impl std::str::FromStr for JobState {
    type Err = InvalidJobState;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "dead" => Ok(Self::Dead),
            _ => Err(InvalidJobState(value.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{JobKind, JobState};

    #[test]
    fn queued_wire_kinds_round_trip() {
        for kind in [JobKind::WorkFetchRequested, JobKind::RecomputeMetrics,
            JobKind::ImportDeduplication, JobKind::PmidImport, JobKind::AutomationRun,
            JobKind::AssistantAgentRun, JobKind::WorkflowStep, JobKind::RecomputePrisma,
            JobKind::RetrieveDocument, JobKind::ParseDocument] {
            assert_eq!(kind.as_str().parse::<JobKind>().unwrap(), kind);
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
        }
    }

    #[test]
    fn invalid_persisted_kind_is_an_error() {
        assert!("not_a_job".parse::<JobKind>().is_err());
        assert!(serde_json::from_str::<JobKind>("\"not_a_job\"").is_err());
    }

    #[test]
    fn durable_job_states_round_trip_and_reject_invalid_values() {
        for state in [JobState::Queued, JobState::Running, JobState::Completed,
            JobState::Failed, JobState::Dead] {
            assert_eq!(state.as_str().parse::<JobState>().unwrap(), state);
        }
        for invalid in ["", "complete", "RUNNING", " queued"] {
            assert!(invalid.parse::<JobState>().is_err());
        }
    }
}
