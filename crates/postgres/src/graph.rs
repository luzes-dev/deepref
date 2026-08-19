use deepref_graph::{GraphEdge, GraphNode, ProjectGraph};
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub const MAX_GRAPH_NODES: i64 = 2_000;

/// Loads the bounded project graph from canonical UUID rows. This adapter owns
/// SQL and ordering; the graph crate only contains in-memory data and analysis.
pub async fn load_project_graph(pool: &PgPool, project_id: Uuid) -> anyhow::Result<ProjectGraph> {
    let rows = sqlx::query(
        r#"SELECT pr.report_id, doi.value AS doi, r.title, r.publication_year,
                  r.total_citations, r.references_count, r.work_type, r.publisher,
                  r.container_title, r.url, pr.internal_citations,
                  pr.outbound_internal_references, pr.rank_score, pr.metrics_computed_at
           FROM project_reports pr
           JOIN reports r ON r.id = pr.report_id
           LEFT JOIN LATERAL (
             SELECT value FROM report_identifiers
             WHERE report_id = pr.report_id AND scheme = 'doi'
             ORDER BY created_at, id
             LIMIT 1
           ) doi ON true
           WHERE pr.project_id = $1
           ORDER BY pr.report_id
           LIMIT $2"#,
    )
    .bind(project_id)
    .bind(MAX_GRAPH_NODES + 1)
    .fetch_all(pool)
    .await?;

    let truncated = rows.len() > MAX_GRAPH_NODES as usize;
    let nodes = rows
        .into_iter()
        .take(MAX_GRAPH_NODES as usize)
        .map(|row| GraphNode {
            report_id: row.get("report_id"),
            doi: row.get("doi"),
            title: row.get("title"),
            issued_year: row.get("publication_year"),
            published_year: None,
            work_type: row.get("work_type"),
            publisher: row.get("publisher"),
            container_title: row.get("container_title"),
            url: row.get("url"),
            total_citations: row.get("total_citations"),
            references_count: row.get("references_count"),
            internal_citations: row.get("internal_citations"),
            outbound_internal_references: row.get("outbound_internal_references"),
            rank_score: row.get("rank_score"),
            metrics_as_of: row.get("metrics_computed_at"),
        })
        .collect::<Vec<_>>();
    let ids = nodes.iter().map(|node| node.report_id).collect::<Vec<_>>();
    let edge_rows = if ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query(
            r#"SELECT source_report_id, target_report_id
               FROM citations
               WHERE project_id = $1
                 AND source_report_id = ANY($2)
                 AND target_report_id = ANY($2)
               ORDER BY source_report_id, target_report_id
               LIMIT $3"#,
        )
        .bind(project_id)
        .bind(&ids)
        .bind(MAX_GRAPH_NODES + 1)
        .fetch_all(pool)
        .await?
    };
    let edge_truncated = edge_rows.len() > MAX_GRAPH_NODES as usize;
    let edges = edge_rows
        .into_iter()
        .take(MAX_GRAPH_NODES as usize)
        .map(|row| GraphEdge {
            source: row.get("source_report_id"),
            target: row.get("target_report_id"),
        })
        .collect();

    Ok(ProjectGraph {
        nodes,
        edges,
        truncated: truncated || edge_truncated,
    })
}

pub async fn recompute_project_metrics(pool: &PgPool, project_id: Uuid) -> anyhow::Result<()> {
    sqlx::query("SELECT recompute_project_report_metrics($1)")
        .bind(project_id)
        .execute(pool)
        .await?;
    Ok(())
}
