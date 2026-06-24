use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::Reference;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FetchStatus {
    Stub,
    Fetched,
    NotFound,
    Failed,
}

impl FetchStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stub => "stub",
            Self::Fetched => "fetched",
            Self::NotFound => "not_found",
            Self::Failed => "failed",
        }
    }
}

impl TryFrom<&str> for FetchStatus {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "stub" => Ok(Self::Stub),
            "fetched" => Ok(Self::Fetched),
            "not_found" => Ok(Self::NotFound),
            "failed" => Ok(Self::Failed),
            _ => Err(format!("unknown fetch status: {value}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Work {
    pub doi: String,
    pub title: Option<String>,
    pub abstract_text: Option<String>,
    pub work_type: Option<String>,
    pub publisher: Option<String>,
    pub container_title: Option<String>,
    pub issued_year: Option<i32>,
    pub published_year: Option<i32>,
    pub url: Option<String>,
    pub total_citations: i32,
    pub references_count: i32,
    pub metadata_provider: String,
    pub citation_provider: String,
    pub fetched_at: DateTime<Utc>,
    pub fetch_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkWithReferences {
    pub work: Work,
    pub references: Vec<Reference>,
    pub raw: serde_json::Value,
}
