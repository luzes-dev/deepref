use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IngestionStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl IngestionStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl TryFrom<&str> for IngestionStatus {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(format!("unknown ingestion status: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IngestionItemStatus {
    Queued,
    Fetching,
    Fetched,
    Skipped,
    NotFound,
    Failed,
}

impl IngestionItemStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Fetching => "fetching",
            Self::Fetched => "fetched",
            Self::Skipped => "skipped",
            Self::NotFound => "not_found",
            Self::Failed => "failed",
        }
    }
}

impl TryFrom<&str> for IngestionItemStatus {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "queued" => Ok(Self::Queued),
            "fetching" => Ok(Self::Fetching),
            "fetched" => Ok(Self::Fetched),
            "skipped" => Ok(Self::Skipped),
            "not_found" => Ok(Self::NotFound),
            "failed" => Ok(Self::Failed),
            _ => Err(format!("unknown ingestion item status: {value}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ingestion {
    pub id: Uuid,
    pub project_id: Uuid,
    pub status: IngestionStatus,
    pub max_depth: i32,
    pub seed_count: i32,
    pub queued_count: i32,
    pub fetched_count: i32,
    pub failed_count: i32,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_strings() {
        assert_eq!(
            IngestionStatus::try_from("cancelled"),
            Ok(IngestionStatus::Cancelled)
        );
        assert_eq!(
            IngestionItemStatus::try_from("not_found"),
            Ok(IngestionItemStatus::NotFound)
        );
        assert!(IngestionItemStatus::try_from("done").is_err());
    }
}
