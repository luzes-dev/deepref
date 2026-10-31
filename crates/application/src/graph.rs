//! Broker-independent graph projection invalidation contract for 04a/04b.
use crate::outbox::OutboxMessage;
use deepref_domain::ProjectId;
use serde::{Deserialize, Serialize};

pub const METRICS_PROJECTION_CONSUMER: &str = "graph_metrics_queue";

/// A hint to reload canonical state, never a copy of the graph or a promise
/// that this event's revision is still current when the worker executes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricsRecomputeRequested {
    pub project_id: ProjectId,
    pub revision: i64,
}

impl OutboxMessage for MetricsRecomputeRequested {
    const TYPE: &'static str = "graph.metrics_recompute_requested";
    const VERSION: u32 = 1;
    const CONSUMERS: &'static [&'static str] = &[METRICS_PROJECTION_CONSUMER];
}
