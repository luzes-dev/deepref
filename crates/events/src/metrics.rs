use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsRecomputeRequested {
    pub project_id: Uuid,
    pub ingestion_id: Option<Uuid>,
}
