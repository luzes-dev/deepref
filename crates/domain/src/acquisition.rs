use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AcquisitionRunId(Uuid);

impl AcquisitionRunId {
    pub const fn new(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl From<Uuid> for AcquisitionRunId {
    fn from(value: Uuid) -> Self {
        Self::new(value)
    }
}

impl From<AcquisitionRunId> for Uuid {
    fn from(value: AcquisitionRunId) -> Self {
        value.as_uuid()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcquisitionStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl AcquisitionStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcquisitionSource {
    Import,
    Crossref,
    Pubmed,
    OpenAlex,
    Manual,
    Other(String),
}

impl AcquisitionSource {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Import => "import",
            Self::Crossref => "crossref",
            Self::Pubmed => "pubmed",
            Self::OpenAlex => "openalex",
            Self::Manual => "manual",
            Self::Other(value) => value,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportFormat {
    Doi,
    Ris,
    Bibtex,
    Nbib,
    Csv,
}

impl ImportFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Doi => "doi",
            Self::Ris => "ris",
            Self::Bibtex => "bibtex",
            Self::Nbib => "nbib",
            Self::Csv => "csv",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AcquisitionStatus;

    #[test]
    fn acquisition_status_rejects_invalid_persisted_values() {
        for value in ["", "RUNNING", "complete", " queued"] {
            assert_eq!(AcquisitionStatus::parse(value), None);
        }
        for status in [
            AcquisitionStatus::Queued,
            AcquisitionStatus::Running,
            AcquisitionStatus::Completed,
            AcquisitionStatus::Failed,
            AcquisitionStatus::Cancelled,
        ] {
            assert_eq!(AcquisitionStatus::parse(status.as_str()), Some(status));
        }
    }
}
