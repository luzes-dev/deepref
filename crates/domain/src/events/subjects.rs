pub const SUBJECT_WORK_FETCH_REQUESTED: &str = "work.fetch.requested.v1";
pub const SUBJECT_WORK_UPSERTED: &str = "domain.work.upserted.v1";
pub const SUBJECT_PROJECT_MEMBERSHIP_UPSERTED: &str = "domain.project_membership.upserted.v1";
pub const SUBJECT_CITATION_UPSERTED: &str = "domain.citation.upserted.v1";
pub const SUBJECT_UNRESOLVED_REFERENCE_UPSERTED: &str = "domain.unresolved_reference.upserted.v1";
pub const SUBJECT_PROJECT_TOMBSTONED: &str = "domain.project.tombstoned.v1";
pub const SUBJECT_METRICS_RECOMPUTE_REQUESTED: &str = "domain.metrics.recompute.requested.v1";
pub const SUBJECT_DLQ: &str = "dlq.recorded.v1";

pub const DELIVERY_BACKOFF_SECONDS: [u64; 5] = [5, 30, 120, 600, 1_800];
pub const MAX_DELIVERIES: u64 = DELIVERY_BACKOFF_SECONDS.len() as u64;
