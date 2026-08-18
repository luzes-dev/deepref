use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::MetricsRecomputeRequested;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum DomainPayload {
    WorkUpserted(WorkUpserted),
    WorkTombstoned(WorkTombstoned),
    ProjectMembershipUpserted(ProjectMembershipUpserted),
    ProjectMembershipTombstoned(ProjectMembershipTombstoned),
    CitationUpserted(CitationUpserted),
    CitationTombstoned(CitationTombstoned),
    UnresolvedReferenceUpserted(UnresolvedReferenceUpserted),
    UnresolvedReferenceTombstoned(UnresolvedReferenceTombstoned),
    ProjectTombstoned(ProjectTombstoned),
    MetricsRecomputeRequested(MetricsRecomputeRequested),
    MetricsUpdated(MetricsUpdated),
    ProjectionCompleted(ProjectionCompleted),
    ProjectionFailed(ProjectionFailed),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkUpserted {
    pub doi: String,
    pub title: Option<String>,
    pub issued_year: Option<i32>,
    pub total_citations: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkTombstoned {
    pub doi: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMembershipUpserted {
    pub project_id: Uuid,
    pub doi: String,
    pub seed: bool,
    pub min_depth: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMembershipTombstoned {
    pub project_id: Uuid,
    pub doi: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CitationUpserted {
    pub project_id: Uuid,
    pub source_doi: String,
    pub target_doi: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CitationTombstoned {
    pub project_id: Uuid,
    pub source_doi: String,
    pub target_doi: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnresolvedReferenceUpserted {
    pub id: String,
    pub project_id: Uuid,
    pub source_doi: String,
    pub raw_unstructured: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnresolvedReferenceTombstoned {
    pub id: String,
    pub project_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectTombstoned {
    pub project_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricsUpdated {
    pub project_id: Uuid,
    pub metrics_as_of: DateTime<Utc>,
    pub work_count: i64,
    pub edge_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectionCompleted {
    pub projection: String,
    pub project_id: Option<Uuid>,
    pub revision: i64,
    pub lag: i64,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectionFailed {
    pub projection: String,
    pub project_id: Option<Uuid>,
    pub revision: i64,
    pub error_code: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_tag_is_stable() {
        let value = serde_json::to_value(DomainPayload::WorkTombstoned(WorkTombstoned {
            doi: "10.1/x".into(),
        }))
        .unwrap();
        assert_eq!(value["kind"], "work_tombstoned");
    }
}
