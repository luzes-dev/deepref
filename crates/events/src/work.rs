use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkFetchRequested {
    pub project_id: Uuid,
    pub ingestion_id: Uuid,
    pub doi: String,
    pub depth: i32,
    pub max_depth: i32,
    pub parent_doi: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkFetchCompleted {
    pub project_id: Uuid,
    pub ingestion_id: Uuid,
    pub doi: String,
    pub references_discovered: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkFetchFailed {
    pub project_id: Uuid,
    pub ingestion_id: Uuid,
    pub doi: String,
    pub error: String,
    pub retryable: bool,
}
