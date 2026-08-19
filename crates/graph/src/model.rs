use chrono::{DateTime, Utc};
use petgraph::{graph::DiGraph, visit::EdgeRef};
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
    pub total_citations: i64,
    pub references_count: i64,
    pub internal_citations: i64,
    pub outbound_internal_references: i64,
    pub rank_score: f64,
    pub metrics_as_of: Option<DateTime<Utc>>,
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
    let mut graph = DiGraph::<Uuid, ()>::new();
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
