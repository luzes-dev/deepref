use chrono::{DateTime, Utc};
use deepref_domain::ProjectId;
use deepref_domain::graph::{
    GraphAppraisalOverlay, GraphEdge, GraphFieldSelection, GraphMetricsOverlay, GraphNode,
    GraphProvenanceOverlay, GraphScreeningOverlay, GraphStudyOverlay, ProjectGraph,
    ProjectionState,
};
use sqlx::{PgConnection, PgPool, Postgres, Row, Transaction};
use std::collections::HashMap;
use uuid::Uuid;

pub const MAX_GRAPH_NODES: i64 = 2_000;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectionStatus {
    pub project_id: Uuid,
    pub state: ProjectionState,
    pub requested_revision: i64,
    pub applied_revision: i64,
    pub revision: i64,
    pub watermark: i64,
    pub lag: i64,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub rebuild_state: Option<String>,
}

/// Loads the bounded project graph from canonical UUID rows. This adapter owns
/// SQL and ordering; the graph crate only contains in-memory data and analysis.
pub async fn load_project_graph(
    pool: &PgPool,
    project_id: Uuid,
    fields: GraphFieldSelection,
) -> anyhow::Result<ProjectGraph> {
    load_project_graph_with_limit(pool, project_id, fields, Some(MAX_GRAPH_NODES)).await
}

/// Strict read-after-write graph loader: ensures graph projection is synchronized with
/// canonical mutations (or recomputed if lagging) before returning, guaranteeing
/// snapshot consistency and avoiding hidden stale state.
/// Strict read loader: checks projection freshness and returns the bounded project graph
/// along with truthful projection status. If the read model lags canonical mutations,
/// it returns explicit lagging/pending status rather than hiding stale state or
/// blocking the HTTP request with synchronous unbounded graph computation.
pub async fn load_project_graph_strict(
    pool: &PgPool,
    project_id: Uuid,
    fields: GraphFieldSelection,
) -> anyhow::Result<(ProjectGraph, ProjectionStatus)> {
    let status = get_projection_status(pool, project_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("project not found: {project_id}"))?;
    let graph = load_project_graph(pool, project_id, fields).await?;
    Ok((graph, status))
}

/// Query projection status for a project with strict tenant scoping.
/// Eliminates global NULL-project row leakage and derives truthful state
/// (pending, ready, lagging, rebuilding, failed) with requested vs applied revision.
pub async fn get_projection_status(
    pool: &PgPool,
    project_id: Uuid,
) -> anyhow::Result<Option<ProjectionStatus>> {
    let project_row = sqlx::query("SELECT metric_requested_revision FROM projects WHERE id = $1")
        .bind(project_id)
        .fetch_optional(pool)
        .await?;

    let Some(project) = project_row else {
        return Ok(None);
    };
    let requested_revision: i64 = project.get("metric_requested_revision");

    let row = sqlx::query(
        "SELECT state, revision, watermark, lag, last_success_at, last_error, rebuild_state \
         FROM projection_state \
         WHERE projection_name = 'postgres_graph' AND project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?;

    if let Some(row) = row {
        let applied_revision: i64 = row.get("watermark");
        let lag = (requested_revision - applied_revision).max(0);
        let rebuild_state: Option<String> = row.get("rebuild_state");
        let last_error: Option<String> = row.get("last_error");
        let last_success_at: Option<DateTime<Utc>> = row.get("last_success_at");
        let persisted_state: String = row.get("state");

        let state = if rebuild_state.is_some() || persisted_state == "rebuilding" {
            ProjectionState::Rebuilding
        } else if last_error.is_some() || persisted_state == "failed" {
            ProjectionState::Failed
        } else if lag > 0 {
            ProjectionState::Lagging
        } else if last_success_at.is_none() || persisted_state == "pending" {
            // Never mark revision 0 fresh absent actual successful projection!
            ProjectionState::Pending
        } else {
            ProjectionState::Ready
        };

        Ok(Some(ProjectionStatus {
            project_id,
            state,
            requested_revision,
            applied_revision,
            revision: applied_revision,
            watermark: applied_revision,
            lag,
            last_success_at,
            last_error,
            rebuild_state,
        }))
    } else {
        let lag = requested_revision;
        let state = if requested_revision > 0 {
            ProjectionState::Lagging
        } else {
            ProjectionState::Pending
        };
        Ok(Some(ProjectionStatus {
            project_id,
            state,
            requested_revision,
            applied_revision: 0,
            revision: 0,
            watermark: 0,
            lag,
            last_success_at: None,
            last_error: None,
            rebuild_state: None,
        }))
    }
}

/// Administrative / operator rebuild path for a project projection.
/// Resets read model state and enqueues a durable recompute job from canonical tables.
pub async fn rebuild_project_projection(pool: &PgPool, project_id: Uuid) -> anyhow::Result<Uuid> {
    let mut transaction = pool.begin().await?;
    let project_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id = $1)")
            .bind(project_id)
            .fetch_one(&mut *transaction)
            .await?;
    if !project_exists {
        anyhow::bail!("project not found: {project_id}");
    }

    let updated = sqlx::query(
        "UPDATE projection_state
         SET state = 'rebuilding', rebuild_state = 'rebuilding', updated_at = now()
         WHERE projection_name = 'postgres_graph' AND project_id = $1",
    )
    .bind(project_id)
    .execute(&mut *transaction)
    .await?;

    if updated.rows_affected() == 0 {
        sqlx::query(
            "INSERT INTO projection_state
               (projection_name, project_id, state, revision, watermark, lag, rebuild_state, updated_at)
             VALUES ('postgres_graph', $1, 'rebuilding', 0, 0, 0, 'rebuilding', now())",
        )
        .bind(project_id)
        .execute(&mut *transaction)
        .await?;
    }

    sqlx::query(
        "UPDATE project_reports
         SET metrics_computed_at = NULL
         WHERE project_id = $1",
    )
    .bind(project_id)
    .execute(&mut *transaction)
    .await?;

    sqlx::query("DELETE FROM metric_snapshots WHERE project_id = $1")
        .bind(project_id)
        .execute(&mut *transaction)
        .await?;

    let job_id = enqueue_metrics_recompute(&mut transaction, project_id).await?;
    transaction.commit().await?;
    Ok(job_id)
}

async fn load_project_graph_with_limit(
    pool: &PgPool,
    project_id: Uuid,
    fields: GraphFieldSelection,
    max_nodes: Option<i64>,
) -> anyhow::Result<ProjectGraph> {
    let mut connection = pool.acquire().await?;
    load_project_graph_from_connection(&mut connection, project_id, fields, max_nodes).await
}

async fn load_project_graph_from_connection(
    connection: &mut PgConnection,
    project_id: Uuid,
    fields: GraphFieldSelection,
    max_nodes: Option<i64>,
) -> anyhow::Result<ProjectGraph> {
    let rows = sqlx::query(
        r#"SELECT pr.report_id, doi.value AS doi, r.title, r.publication_year,
                  r.work_type, r.publisher, r.container_title, r.url,
                  r.total_citations, r.references_count, pr.internal_citations,
                  pr.outbound_internal_references, pr.rank_score, pr.metrics_computed_at,
                  (pr.metrics_computed_at IS NULL OR
                   pr.metrics_computed_at < now() - interval '1 hour') AS metrics_stale
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
    .bind(max_nodes.map(|limit| limit + 1))
    .fetch_all(&mut *connection)
    .await?;

    let node_limit = max_nodes.map_or(rows.len(), |limit| limit as usize);
    let truncated = max_nodes.is_some_and(|_| rows.len() > node_limit);
    let mut nodes = rows
        .into_iter()
        .take(node_limit)
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
            metrics: fields.metrics.then_some(GraphMetricsOverlay {
                total_citations: row.get("total_citations"),
                references_count: row.get("references_count"),
                internal_citations: row.get("internal_citations"),
                outbound_internal_references: row.get("outbound_internal_references"),
                rank_score: row.get("rank_score"),
                metrics_as_of: row.get("metrics_computed_at"),
                metrics_stale: row.get("metrics_stale"),
            }),
            screening: None,
            study: None,
            appraisal: None,
            provenance: None,
        })
        .collect::<Vec<_>>();
    let ids = nodes.iter().map(|node| node.report_id).collect::<Vec<_>>();

    let node_indices = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.report_id, index))
        .collect::<HashMap<_, _>>();
    if fields.screening {
        attach_screening_overlays(connection, project_id, &ids, &node_indices, &mut nodes).await?;
    }
    if fields.study {
        attach_study_overlays(connection, project_id, &ids, &node_indices, &mut nodes).await?;
    }
    if fields.appraisal {
        attach_appraisal_overlays(connection, project_id, &ids, &node_indices, &mut nodes).await?;
    }
    if fields.provenance {
        attach_provenance_overlays(connection, project_id, &ids, &node_indices, &mut nodes).await?;
    }
    let (edges, edge_truncated) = load_graph_edges(connection, project_id, &ids, max_nodes).await?;

    Ok(ProjectGraph {
        nodes,
        edges,
        truncated: truncated || edge_truncated,
    })
}

/// Queues a full-project metrics recomputation in the caller's transaction, so
/// it commits or rolls back with the change that made citation counts stale.
/// The worker recomputes every report in the project, so one request per
/// committed change is enough.
pub async fn enqueue_metrics_recompute(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
) -> Result<Uuid, sqlx::Error> {
    let revision: i64 = sqlx::query_scalar("UPDATE projects SET metric_requested_revision=metric_requested_revision+1 WHERE id=$1 RETURNING metric_requested_revision")
        .bind(project_id)
        .fetch_one(&mut **transaction)
        .await?;
    let job_id = Uuid::new_v4();
    let payload = serde_json::json!({"project_id": project_id, "revision": revision});
    crate::jobs::enqueue_job(
        transaction,
        &crate::jobs::job(
            job_id,
            ProjectId::new(project_id),
            deepref_domain::JobKind::RecomputeMetrics,
            payload,
            format!("recompute_metrics:{project_id}:{revision}"),
        ),
    )
    .await
    .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
    Ok(job_id)
}

pub async fn recompute_project_metrics(pool: &PgPool, project_id: Uuid) -> anyhow::Result<()> {
    crate::with_retry(crate::RetryPolicy::default(), || {
        recompute_project_metrics_once(pool, project_id)
    })
    .await
}

async fn recompute_project_metrics_once(pool: &PgPool, project_id: Uuid) -> anyhow::Result<()> {
    // The repository load is the only source for the in-memory graph. The
    // public loader is bounded for HTTP responses; recomputation deliberately
    // loads the complete project so persisted metrics cover every report.
    let mut transaction = pool.begin().await?;
    // The complete graph, revision, timestamp, and writes must share one
    // repeatable snapshot. A concurrent canonical citation/report change then
    // either remains outside this recomputation or causes the transaction to
    // retry/fail, never producing a mixed graph projection.
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *transaction)
        .await?;
    // Serialize recomputations and graph-change requests on the project row.
    // Under repeatable read a changed lock target forces a retry rather than
    // allowing an older snapshot to overwrite a newer metric projection.
    let current_revision: i64 =
        sqlx::query_scalar("SELECT metric_requested_revision FROM projects WHERE id=$1 FOR UPDATE")
            .bind(project_id)
            .fetch_one(&mut *transaction)
            .await?;
    let graph = load_project_graph_from_connection(
        &mut transaction,
        project_id,
        GraphFieldSelection::metrics(),
        None,
    )
    .await?;
    let (computed_at, current_year): (DateTime<Utc>, i32) =
        sqlx::query_as("SELECT now(), EXTRACT(YEAR FROM CURRENT_DATE)::int")
            .fetch_one(&mut *transaction)
            .await?;
    let metrics = deepref_domain::graph::compute_metrics(&graph.nodes, &graph.edges, current_year);
    for node in &graph.nodes {
        let metric = metrics
            .get(&node.report_id)
            .ok_or_else(|| anyhow::anyhow!("every repository node has computed graph metrics"))?;
        sqlx::query(
            "UPDATE project_reports
             SET total_citations=$1, references_count=$2,
                 internal_citations=$3, outbound_internal_references=$4,
                 rank_score=$5, metrics_computed_at=$6
             WHERE project_id=$7 AND report_id=$8",
        )
        .bind(metric.total_citations)
        .bind(metric.references_count)
        .bind(metric.internal_citations)
        .bind(metric.outbound_internal_references)
        .bind(metric.rank_score)
        .bind(computed_at)
        .bind(project_id)
        .bind(node.report_id)
        .execute(&mut *transaction)
        .await?;
    }

    let work_count = i64::try_from(graph.nodes.len())?;
    let edge_count = i64::try_from(graph.edges.len())?;
    sqlx::query(
        "INSERT INTO metric_snapshots
           (project_id, revision, metrics_as_of, work_count, edge_count, payload)
         VALUES ($1,$2,$3,$4,$5,$6)
         ON CONFLICT (project_id, revision) DO UPDATE SET
           metrics_as_of=EXCLUDED.metrics_as_of,
           work_count=EXCLUDED.work_count,
           edge_count=EXCLUDED.edge_count,
           payload=EXCLUDED.payload",
    )
    .bind(project_id)
    .bind(current_revision)
    .bind(computed_at)
    .bind(work_count)
    .bind(edge_count)
    .bind(serde_json::json!({"work_count": work_count, "edge_count": edge_count}))
    .execute(&mut *transaction)
    .await?;

    let projection_update = sqlx::query(
        "UPDATE projection_state
         SET state='ready', revision=$1, watermark=GREATEST(COALESCE(watermark, 0), $1), lag=0,
             last_success_at=$2, last_error=NULL, rebuild_state=NULL, updated_at=$2
         WHERE projection_name='postgres_graph' AND project_id=$3 AND watermark <= $1",
    )
    .bind(current_revision)
    .bind(computed_at)
    .bind(project_id)
    .execute(&mut *transaction)
    .await?;

    if projection_update.rows_affected() == 0 {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM projection_state WHERE projection_name='postgres_graph' AND project_id=$1)",
        )
        .bind(project_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !exists {
            sqlx::query(
                "INSERT INTO projection_state
                   (projection_name, project_id, state, revision, watermark, lag, last_success_at, updated_at)
                 VALUES ('postgres_graph',$1,'ready',$2,$2,0,$3,$3)",
            )
            .bind(project_id)
            .bind(current_revision)
            .bind(computed_at)
            .execute(&mut *transaction)
            .await?;
        }
    }
    transaction.commit().await?;
    Ok(())
}

async fn attach_screening_overlays(
    connection: &mut PgConnection,
    project_id: Uuid,
    ids: &[Uuid],
    node_indices: &HashMap<Uuid, usize>,
    nodes: &mut [GraphNode],
) -> anyhow::Result<()> {
    for node in nodes.iter_mut() {
        node.screening = Some(GraphScreeningOverlay {
            title_abstract_status: "unscreened".to_owned(),
            full_text_status: "not_required".to_owned(),
            final_status: "unscreened".to_owned(),
        });
    }
    let rows = sqlx::query(
        "SELECT report_id,title_abstract_status,full_text_status,final_status FROM screening_state WHERE project_id=$1 AND report_id=ANY($2) AND NOT ai_first_audit_masked(project_id,report_id) ORDER BY report_id",
    )
    .bind(project_id)
    .bind(ids)
    .fetch_all(&mut *connection)
    .await?;
    for row in rows {
        if let Some(&index) = node_indices.get(&row.get::<Uuid, _>("report_id")) {
            nodes[index].screening = Some(GraphScreeningOverlay {
                title_abstract_status: row.get("title_abstract_status"),
                full_text_status: row.get("full_text_status"),
                final_status: row.get("final_status"),
            });
        }
    }
    Ok(())
}

async fn attach_study_overlays(
    connection: &mut PgConnection,
    project_id: Uuid,
    ids: &[Uuid],
    node_indices: &HashMap<Uuid, usize>,
    nodes: &mut [GraphNode],
) -> anyhow::Result<()> {
    for node in nodes.iter_mut() {
        node.study = Some(GraphStudyOverlay {
            study_id: None,
            title: None,
        });
    }
    let rows = sqlx::query(
        r#"SELECT DISTINCT ON (sr.report_id) sr.report_id, s.id AS study_id, s.title
           FROM study_reports sr
           JOIN studies s ON s.project_id=sr.project_id AND s.id=sr.study_id
           WHERE sr.project_id=$1 AND sr.report_id=ANY($2)
           ORDER BY sr.report_id, s.id"#,
    )
    .bind(project_id)
    .bind(ids)
    .fetch_all(&mut *connection)
    .await?;
    for row in rows {
        if let Some(&index) = node_indices.get(&row.get::<Uuid, _>("report_id")) {
            nodes[index].study = Some(GraphStudyOverlay {
                study_id: row.get("study_id"),
                title: row.get("title"),
            });
        }
    }
    Ok(())
}

async fn attach_appraisal_overlays(
    connection: &mut PgConnection,
    project_id: Uuid,
    ids: &[Uuid],
    node_indices: &HashMap<Uuid, usize>,
    nodes: &mut [GraphNode],
) -> anyhow::Result<()> {
    for node in nodes.iter_mut() {
        node.appraisal = Some(GraphAppraisalOverlay {
            assessment_count: 0,
            completed_count: 0,
            latest_completed_at: None,
        });
    }
    let rows = sqlx::query(
        "SELECT report_id,count(*)::bigint AS assessment_count,count(*)::bigint AS completed_count,max(completed_at) AS latest_completed_at FROM appraisal_assessments WHERE project_id=$1 AND report_id=ANY($2) GROUP BY report_id ORDER BY report_id",
    )
    .bind(project_id)
    .bind(ids)
    .fetch_all(&mut *connection)
    .await?;
    for row in rows {
        if let Some(&index) = node_indices.get(&row.get::<Uuid, _>("report_id")) {
            nodes[index].appraisal = Some(GraphAppraisalOverlay {
                assessment_count: row.get("assessment_count"),
                completed_count: row.get("completed_count"),
                latest_completed_at: row.get("latest_completed_at"),
            });
        }
    }
    Ok(())
}

async fn attach_provenance_overlays(
    connection: &mut PgConnection,
    project_id: Uuid,
    ids: &[Uuid],
    node_indices: &HashMap<Uuid, usize>,
    nodes: &mut [GraphNode],
) -> anyhow::Result<()> {
    for node in nodes.iter_mut() {
        node.provenance = Some(GraphProvenanceOverlay {
            sources: Vec::new(),
            source_record_count: 0,
        });
    }
    let rows = sqlx::query(
        "SELECT report_id,array_agg(DISTINCT source ORDER BY source) AS sources,count(*)::bigint AS source_record_count FROM records WHERE project_id=$1 AND report_id=ANY($2) GROUP BY report_id ORDER BY report_id",
    )
    .bind(project_id)
    .bind(ids)
    .fetch_all(&mut *connection)
    .await?;
    for row in rows {
        if let Some(&index) = node_indices.get(&row.get::<Uuid, _>("report_id")) {
            nodes[index].provenance = Some(GraphProvenanceOverlay {
                sources: row.get("sources"),
                source_record_count: row.get("source_record_count"),
            });
        }
    }
    Ok(())
}

async fn load_graph_edges(
    connection: &mut PgConnection,
    project_id: Uuid,
    ids: &[Uuid],
    max_nodes: Option<i64>,
) -> anyhow::Result<(Vec<GraphEdge>, bool)> {
    if ids.is_empty() {
        return Ok((Vec::new(), false));
    }
    let edge_rows = sqlx::query(
        r#"SELECT source_report_id, target_report_id
           FROM citations
           WHERE project_id = $1
             AND source_report_id = ANY($2)
             AND target_report_id = ANY($2)
           ORDER BY source_report_id, target_report_id
           LIMIT $3"#,
    )
    .bind(project_id)
    .bind(ids)
    .bind(max_nodes.map(|limit| limit + 1))
    .fetch_all(&mut *connection)
    .await?;

    let edge_limit = max_nodes.map_or(edge_rows.len(), |limit| limit as usize);
    let edge_truncated = max_nodes.is_some_and(|_| edge_rows.len() > edge_limit);
    let edges = edge_rows
        .into_iter()
        .take(edge_limit)
        .map(|row| GraphEdge {
            source: row.get("source_report_id"),
            target: row.get("target_report_id"),
        })
        .collect();
    Ok((edges, edge_truncated))
}
