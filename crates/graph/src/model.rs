use chrono::{DateTime, Utc};
use petgraph::{
    stable_graph::StableDiGraph,
    visit::{EdgeRef, IntoEdgeReferences},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphNode {
    pub report_id: Uuid,
    pub doi: Option<String>,
    pub title: Option<String>,
    pub issued_year: Option<i32>,
    pub published_year: Option<i32>,
    pub work_type: Option<String>,
    pub publisher: Option<String>,
    pub container_title: Option<String>,
    pub url: Option<String>,
    pub metrics: Option<GraphMetricsOverlay>,
    pub screening: Option<GraphScreeningOverlay>,
    pub study: Option<GraphStudyOverlay>,
    pub appraisal: Option<GraphAppraisalOverlay>,
    pub provenance: Option<GraphProvenanceOverlay>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphMetricsOverlay {
    pub total_citations: i64,
    pub references_count: i64,
    pub internal_citations: i64,
    pub outbound_internal_references: i64,
    pub rank_score: f64,
    pub metrics_as_of: Option<DateTime<Utc>>,
    pub metrics_stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphScreeningOverlay {
    pub title_abstract_status: String,
    pub full_text_status: String,
    pub final_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphStudyOverlay {
    pub study_id: Option<Uuid>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphAppraisalOverlay {
    pub assessment_count: i64,
    pub completed_count: i64,
    pub latest_completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphProvenanceOverlay {
    pub sources: Vec<String>,
    pub source_record_count: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphFieldSelection {
    pub screening: bool,
    pub metrics: bool,
    pub study: bool,
    pub appraisal: bool,
    pub provenance: bool,
}

impl GraphFieldSelection {
    pub const fn metrics() -> Self {
        Self {
            screening: false,
            metrics: true,
            study: false,
            appraisal: false,
            provenance: false,
        }
    }

    pub const fn all() -> Self {
        Self {
            screening: true,
            metrics: true,
            study: true,
            appraisal: true,
            provenance: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub source: Uuid,
    pub target: Uuid,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphDegree {
    pub internal_citations: i64,
    pub outbound_internal_references: i64,
}

/// Runs graph algorithms in memory only. PostgreSQL remains the source of truth;
/// petgraph indices never cross this boundary or get persisted.
pub fn degree_metrics(nodes: &[GraphNode], edges: &[GraphEdge]) -> HashMap<Uuid, GraphDegree> {
    let mut graph = StableDiGraph::<Uuid, ()>::new();
    let indices = nodes
        .iter()
        .map(|node| (node.report_id, graph.add_node(node.report_id)))
        .collect::<HashMap<_, _>>();
    for edge in edges {
        if let (Some(&source), Some(&target)) =
            (indices.get(&edge.source), indices.get(&edge.target))
        {
            graph.add_edge(source, target, ());
        }
    }
    let mut metrics = nodes
        .iter()
        .map(|node| {
            (
                node.report_id,
                GraphDegree {
                    internal_citations: 0,
                    outbound_internal_references: 0,
                },
            )
        })
        .collect::<HashMap<_, _>>();
    for edge in graph.edge_references() {
        let source = graph[edge.source()];
        let target = graph[edge.target()];
        if let Some(degree) = metrics.get_mut(&source) {
            degree.outbound_internal_references += 1;
        }
        if let Some(degree) = metrics.get_mut(&target) {
            degree.internal_citations += 1;
        }
    }
    metrics
}

/// Computes the persisted graph metrics from a repository-loaded UUID graph.
///
/// The graph is materialized with `StableDiGraph` inside `degree_metrics`; the
/// returned map is keyed by the durable report UUIDs, never by petgraph's
/// temporary node indices. `rank_score` deliberately mirrors the legacy
/// DeepRef formula so existing ranking fixtures remain stable while the graph
/// itself becomes the computation path.
pub fn compute_metrics(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    current_year: i32,
) -> HashMap<Uuid, GraphMetricsOverlay> {
    let degrees = degree_metrics(nodes, edges);
    let max_total = nodes
        .iter()
        .map(|node| {
            node.metrics
                .as_ref()
                .map_or(0, |metrics| metrics.total_citations.max(0))
        })
        .map(|value| (value as f64 + 1.0).log10())
        .fold(1.0, f64::max);
    let max_internal = nodes
        .iter()
        .map(|node| {
            degrees
                .get(&node.report_id)
                .map_or(0, |value| value.internal_citations)
        })
        .max()
        .unwrap_or(0)
        .max(1) as f64;
    let max_outbound = nodes
        .iter()
        .map(|node| {
            degrees
                .get(&node.report_id)
                .map_or(0, |value| value.outbound_internal_references)
        })
        .max()
        .unwrap_or(0)
        .max(1) as f64;

    nodes
        .iter()
        .map(|node| {
            let source = node.metrics.as_ref();
            let total_citations = source.map_or(0, |metrics| metrics.total_citations);
            let references_count = source.map_or(0, |metrics| metrics.references_count);
            let degree = degrees
                .get(&node.report_id)
                .copied()
                .unwrap_or(GraphDegree {
                    internal_citations: 0,
                    outbound_internal_references: 0,
                });
            let recency = node.issued_year.map_or(0.0, |issued_year| {
                let age = (current_year - issued_year).max(0) as f64;
                1.0 / (1.0 + age / 10.0)
            });
            let rank_score = 0.45 * ((total_citations.max(0) as f64 + 1.0).log10() / max_total)
                + 0.40 * (degree.internal_citations as f64 / max_internal)
                + 0.10 * (degree.outbound_internal_references as f64 / max_outbound)
                + 0.05 * recency;
            (
                node.report_id,
                GraphMetricsOverlay {
                    total_citations,
                    references_count,
                    internal_citations: degree.internal_citations,
                    outbound_internal_references: degree.outbound_internal_references,
                    rank_score,
                    metrics_as_of: None,
                    metrics_stale: false,
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(report_id: Uuid) -> GraphNode {
        GraphNode {
            report_id,
            doi: None,
            title: None,
            issued_year: None,
            published_year: None,
            work_type: None,
            publisher: None,
            container_title: None,
            url: None,
            metrics: None,
            screening: None,
            study: None,
            appraisal: None,
            provenance: None,
        }
    }

    #[test]
    fn degree_metrics_are_uuid_keyed_and_ignore_out_of_projection_edges() {
        let source = Uuid::new_v4();
        let target = Uuid::new_v4();
        let outside = Uuid::new_v4();
        let metrics = degree_metrics(
            &[node(source), node(target)],
            &[
                GraphEdge { source, target },
                GraphEdge {
                    source: outside,
                    target,
                },
            ],
        );

        assert_eq!(
            metrics[&source],
            GraphDegree {
                internal_citations: 0,
                outbound_internal_references: 1,
            }
        );
        assert_eq!(
            metrics[&target],
            GraphDegree {
                internal_citations: 1,
                outbound_internal_references: 0,
            }
        );
        assert!(!metrics.contains_key(&outside));
    }

    #[test]
    fn compute_metrics_uses_stable_uuid_graph_and_preserves_rank_formula() {
        let source = Uuid::new_v4();
        let target = Uuid::new_v4();
        let nodes = [
            GraphNode {
                report_id: source,
                issued_year: Some(2024),
                metrics: Some(GraphMetricsOverlay {
                    total_citations: 100,
                    references_count: 2,
                    internal_citations: 0,
                    outbound_internal_references: 0,
                    rank_score: 0.0,
                    metrics_as_of: None,
                    metrics_stale: true,
                }),
                ..node(source)
            },
            GraphNode {
                report_id: target,
                issued_year: Some(2020),
                metrics: Some(GraphMetricsOverlay {
                    total_citations: 10,
                    references_count: 1,
                    internal_citations: 0,
                    outbound_internal_references: 0,
                    rank_score: 0.0,
                    metrics_as_of: None,
                    metrics_stale: true,
                }),
                ..node(target)
            },
        ];
        let computed = compute_metrics(&nodes, &[GraphEdge { source, target }], 2026);
        assert_eq!(computed[&source].outbound_internal_references, 1);
        assert_eq!(computed[&target].internal_citations, 1);
        let expected = 0.45 * ((10_f64 + 1.0).log10() / (101_f64).log10())
            + 0.40
            + 0.05 * (1.0 / (1.0 + 6.0 / 10.0));
        assert!((computed[&target].rank_score - expected).abs() < 1e-12);
    }
}
