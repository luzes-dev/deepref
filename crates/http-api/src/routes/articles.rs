use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use chrono::{DateTime, Utc};
use deepref_core::normalize_doi;
use deepref_events::{
    DomainPayload, EntityType, EventEnvelope, MetricsRecomputeRequested,
    SUBJECT_METRICS_RECOMPUTE_REQUESTED,
};
use serde::Serialize;
use sqlx::Row;
use utoipa::ToSchema;
use uuid::Uuid;

use super::pagination::{PaginatedResponse, PaginationParams, page};
use crate::{
    error::{ApiError, ErrorResponse},
    outbox,
    state::AppState,
};

#[derive(Debug, Serialize, Clone, ToSchema)]
pub(crate) struct ReportDto {
    report_id: Uuid,
    doi: Option<String>,
    title: Option<String>,
    issued_year: Option<i32>,
    #[serde(rename = "type")]
    work_type: Option<String>,
    total_citations: i32,
    internal_citations: i32,
    outbound_internal_references: i32,
    rank_score: f64,
    metrics_as_of: Option<DateTime<Utc>>,
    metrics_stale: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ReportDetailDto {
    report_id: Uuid,
    doi: Option<String>,
    title: Option<String>,
    #[serde(rename = "abstract")]
    abstract_text: Option<String>,
    issued_year: Option<i32>,
    published_year: Option<i32>,
    #[serde(rename = "type")]
    work_type: Option<String>,
    publisher: Option<String>,
    container_title: Option<String>,
    url: Option<String>,
    total_citations: i32,
    references_count: i32,
    metrics_as_of: Option<DateTime<Utc>>,
    metrics_stale: bool,
    #[schema(value_type = Object)]
    raw: serde_json::Value,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct GraphEdgeDto {
    source: Uuid,
    target: Uuid,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ProjectionMetadata {
    revision: i64,
    lag: i64,
    last_success_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ProjectGraphDto {
    nodes: Vec<ReportDto>,
    edges: Vec<GraphEdgeDto>,
    projection: ProjectionMetadata,
    truncated: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RecommendationGroupsDto {
    foundational: Vec<ReportDto>,
    core_to_project: Vec<ReportDto>,
    underexplored: Vec<ReportDto>,
    projection: ProjectionMetadata,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RecomputeMetricsDto {
    status: &'static str,
    project_id: Uuid,
    event_id: Uuid,
}

#[utoipa::path(get, path="/projects/{project_id}/reports", operation_id="listProjectReports", tag="reports",
    params(("project_id"=Uuid, Path), PaginationParams),
    responses((status=200, body=PaginatedResponse<ReportDto>), (status=500, body=ErrorResponse)))]
pub(crate) async fn list_reports(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Query(pagination): Query<PaginationParams>,
) -> Result<Json<PaginatedResponse<ReportDto>>, ApiError> {
    let limit = pagination.limit()?;
    let cursor: Option<(f64, i32, i32, Uuid)> = pagination.decode()?;
    let rows = sqlx::query(
        r#"SELECT r.id AS report_id,doi.value AS doi,
        r.title,COALESCE(r.publication_year,w.issued_year) AS issued_year,w.work_type,
        COALESCE(w.total_citations,0) AS total_citations,
        COALESCE(pw.internal_citations,0) AS internal_citations,
        COALESCE(pw.outbound_internal_references,0) AS outbound_internal_references,
        COALESCE(pw.rank_score,0) AS rank_score,pw.metrics_computed_at,
        (pw.metrics_computed_at IS NULL OR pw.metrics_computed_at < now()-interval '1 hour') AS metrics_stale
        FROM project_reports pr
        JOIN reports r ON r.id=pr.report_id
        LEFT JOIN LATERAL (
          SELECT value,normalized_value
          FROM report_identifiers
          WHERE report_id=r.id AND scheme='doi'
          ORDER BY created_at,id
          LIMIT 1
        ) doi ON TRUE
        LEFT JOIN project_works pw ON pw.project_id=pr.project_id AND pw.canonical_doi=doi.normalized_value
        LEFT JOIN works w ON w.canonical_doi=pw.canonical_doi
        WHERE pr.project_id=$1 AND ($2::float8 IS NULL OR
          (COALESCE(pw.rank_score,0),COALESCE(pw.internal_citations,0),COALESCE(w.total_citations,0),pr.report_id)<($2,$3,$4,$5))
        ORDER BY COALESCE(pw.rank_score,0) DESC,COALESCE(pw.internal_citations,0) DESC,
          COALESCE(w.total_citations,0) DESC,pr.report_id DESC LIMIT $6"#,
    ).bind(project_id).bind(cursor.as_ref().map(|value| value.0))
        .bind(cursor.as_ref().map(|value| value.1)).bind(cursor.as_ref().map(|value| value.2))
        .bind(cursor.as_ref().map(|value| value.3)).bind(limit + 1)
        .fetch_all(&state.pool).await?;
    let graph_stale = match &state.graph {
        Some(graph) => graph.ping().await.is_err(),
        None => true,
    };
    let reports = rows
        .into_iter()
        .map(report_from_row)
        .map(|mut report| {
            report.metrics_stale |= graph_stale;
            report
        })
        .collect();
    Ok(Json(page(reports, limit as usize, |report| {
        (
            report.rank_score,
            report.internal_citations,
            report.total_citations,
            report.report_id,
        )
    })?))
}

#[utoipa::path(get, path="/projects/{project_id}/reports/{report_id}", operation_id="getProjectReport", tag="reports",
    params(("project_id"=Uuid, Path),("report_id"=Uuid, Path)),
    responses((status=200, body=ReportDetailDto),(status=400,body=ErrorResponse),(status=404,body=ErrorResponse)))]
pub(crate) async fn get_report(
    State(state): State<AppState>,
    Path((project_id, report_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ReportDetailDto>, ApiError> {
    let row = sqlx::query(
        r#"SELECT r.id AS report_id,doi.value AS doi,
        COALESCE(r.title,w.title) AS title,COALESCE(r.abstract_text,w.abstract_text) AS abstract_text,
        COALESCE(r.publication_year,w.issued_year) AS issued_year,w.published_year,w.work_type,
        w.publisher,COALESCE(r.journal,w.container_title) AS container_title,COALESCE(r.url,w.url) AS url,
        COALESCE(w.total_citations,0) AS total_citations,COALESCE(w.references_count,0) AS references_count,
        r.raw,pw.metrics_computed_at,
        (pw.metrics_computed_at IS NULL OR pw.metrics_computed_at<now()-interval '1 hour') AS metrics_stale
        FROM project_reports pr
        JOIN reports r ON r.id=pr.report_id
        LEFT JOIN LATERAL (
          SELECT value,normalized_value
          FROM report_identifiers
          WHERE report_id=r.id AND scheme='doi'
          ORDER BY created_at,id
          LIMIT 1
        ) doi ON TRUE
        LEFT JOIN project_works pw ON pw.project_id=pr.project_id AND pw.canonical_doi=doi.normalized_value
        LEFT JOIN works w ON w.canonical_doi=pw.canonical_doi
        WHERE pr.project_id=$1 AND pr.report_id=$2"#,
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_one(&state.pool)
    .await?;
    let graph_stale = match &state.graph {
        Some(graph) => graph.ping().await.is_err(),
        None => true,
    };
    Ok(Json(ReportDetailDto {
        report_id: row.get("report_id"),
        doi: row.get("doi"),
        title: row.get("title"),
        abstract_text: row.get("abstract_text"),
        issued_year: row.get("issued_year"),
        published_year: row.get("published_year"),
        work_type: row.get("work_type"),
        publisher: row.get("publisher"),
        container_title: row.get("container_title"),
        url: row.get("url"),
        total_citations: row.get("total_citations"),
        references_count: row.get("references_count"),
        metrics_as_of: row.get("metrics_computed_at"),
        metrics_stale: row.get::<bool, _>("metrics_stale") || graph_stale,
        raw: row.get("raw"),
    }))
}

#[utoipa::path(get, path="/projects/{project_id}/graph", operation_id="getProjectGraph", tag="reports",
    params(("project_id"=Uuid, Path)),
    responses((status=200,body=ProjectGraphDto),(status=503,body=ErrorResponse)))]
pub(crate) async fn project_graph(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<ProjectGraphDto>, ApiError> {
    let graph = state
        .graph
        .as_ref()
        .ok_or_else(|| ApiError::graph_unavailable(state.graph_retry_after))?;
    let projection = projection_metadata(&state, project_id).await?;
    let graph_data = graph
        .project_graph(
            project_id,
            deepref_graph::ProjectionMetadata {
                revision: projection.revision,
                lag: projection.lag,
                last_success_at: projection.last_success_at,
            },
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, %project_id, "graph query failed");
            ApiError::graph_unavailable(state.graph_retry_after)
        })?;
    let report_mappings = graph_report_mappings(&state, project_id, &graph_data.nodes).await?;
    let nodes = graph_data
        .nodes
        .into_iter()
        .filter_map(|node| {
            let normalized_doi = normalize_doi(&node.doi).ok()?;
            let report = report_mappings.get(&normalized_doi)?;
            Some(ReportDto {
                report_id: report.report_id,
                doi: Some(report.doi.clone()),
                title: report.title.clone().or(node.title),
                issued_year: report
                    .issued_year
                    .or_else(|| node.issued_year.map(|year| year as i32)),
                work_type: None,
                total_citations: node.total_citations as i32,
                internal_citations: 0,
                outbound_internal_references: 0,
                rank_score: 0.0,
                metrics_as_of: projection.last_success_at,
                metrics_stale: projection.lag > 0,
            })
        })
        .collect();
    let edges = graph_data
        .edges
        .into_iter()
        .filter_map(|edge| {
            let source = normalize_doi(&edge.source)
                .ok()
                .and_then(|doi| report_mappings.get(&doi))?;
            let target = normalize_doi(&edge.target)
                .ok()
                .and_then(|doi| report_mappings.get(&doi))?;
            Some(GraphEdgeDto {
                source: source.report_id,
                target: target.report_id,
            })
        })
        .collect();
    Ok(Json(ProjectGraphDto {
        nodes,
        edges,
        projection,
        truncated: graph_data.truncated,
    }))
}

#[utoipa::path(get, path="/projects/{project_id}/recommendations", operation_id="getProjectRecommendations", tag="reports",
    params(("project_id"=Uuid, Path)), responses((status=200,body=RecommendationGroupsDto),(status=503,body=ErrorResponse)))]
pub(crate) async fn recommendations(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<RecommendationGroupsDto>, ApiError> {
    let graph = project_graph(State(state), Path(project_id)).await?.0;
    // The graph query itself is bounded at 2,000 nodes; response groups are
    // explicitly limited and never load an unbounded SQL article set.
    Ok(Json(RecommendationGroupsDto {
        foundational: graph.nodes.iter().take(5).cloned().collect(),
        core_to_project: graph.nodes.iter().skip(5).take(5).cloned().collect(),
        underexplored: graph.nodes.iter().rev().take(5).cloned().collect(),
        projection: graph.projection,
    }))
}

#[utoipa::path(post, path="/projects/{project_id}/metrics/recompute", operation_id="recomputeProjectMetrics", tag="reports",
    params(("project_id"=Uuid, Path)), responses((status=202,body=RecomputeMetricsDto),(status=500,body=ErrorResponse)))]
pub(crate) async fn recompute_metrics(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<(StatusCode, Json<RecomputeMetricsDto>), ApiError> {
    let mut tx = state.pool.begin().await?;
    let revision: i64 = sqlx::query_scalar("SELECT nextval('graph_domain_revision_seq')")
        .fetch_one(&mut *tx)
        .await?;
    let event = EventEnvelope::v1(
        SUBJECT_METRICS_RECOMPUTE_REQUESTED,
        "deepref.api",
        EntityType::Metric,
        project_id.to_string(),
        revision,
        project_id,
        None,
        DomainPayload::MetricsRecomputeRequested(MetricsRecomputeRequested {
            project_id,
            ingestion_id: None,
        }),
    );
    sqlx::query(
        "INSERT INTO domain_events (event_id,schema_version,event_type,entity_type,entity_key,revision,payload,correlation_id,causation_id,created_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    ).bind(event.event_id).bind(event.schema_version as i16).bind(&event.event_type)
        .bind(event.entity_type.as_str()).bind(&event.entity_key).bind(event.revision)
        .bind(serde_json::to_value(&event.payload)?).bind(event.correlation_id)
        .bind(event.causation_id).bind(event.occurred_at).execute(&mut *tx).await?;
    outbox::enqueue(
        &mut tx,
        event.event_id,
        SUBJECT_METRICS_RECOMPUTE_REQUESTED,
        &event,
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(RecomputeMetricsDto {
            status: "queued",
            project_id,
            event_id: event.event_id,
        }),
    ))
}

async fn projection_metadata(
    state: &AppState,
    project_id: Uuid,
) -> Result<ProjectionMetadata, ApiError> {
    let row = sqlx::query(
        "SELECT revision,lag,last_success_at FROM projection_state WHERE projection_name='graph' \
         AND (project_id=$1 OR project_id IS NULL) ORDER BY project_id NULLS LAST LIMIT 1",
    )
    .bind(project_id)
    .fetch_one(&state.pool)
    .await?;
    Ok(ProjectionMetadata {
        revision: row.get("revision"),
        lag: row.get("lag"),
        last_success_at: row.get("last_success_at"),
    })
}

fn report_from_row(row: sqlx::postgres::PgRow) -> ReportDto {
    ReportDto {
        report_id: row.get("report_id"),
        doi: row.get("doi"),
        title: row.get("title"),
        issued_year: row.get("issued_year"),
        work_type: row.get("work_type"),
        total_citations: row.get("total_citations"),
        internal_citations: row.get("internal_citations"),
        outbound_internal_references: row.get("outbound_internal_references"),
        rank_score: row.get("rank_score"),
        metrics_as_of: row.get("metrics_computed_at"),
        metrics_stale: row.get("metrics_stale"),
    }
}

#[derive(Debug, Clone)]
struct GraphReportMapping {
    report_id: Uuid,
    doi: String,
    title: Option<String>,
    issued_year: Option<i32>,
}

async fn graph_report_mappings(
    state: &AppState,
    project_id: Uuid,
    nodes: &[deepref_graph::GraphNode],
) -> Result<HashMap<String, GraphReportMapping>, ApiError> {
    let normalized_dois = nodes
        .iter()
        .filter_map(|node| normalize_doi(&node.doi).ok())
        .collect::<Vec<_>>();
    if normalized_dois.is_empty() {
        return Ok(HashMap::new());
    }

    let rows = sqlx::query(
        r#"SELECT requested.normalized_doi,r.id AS report_id,ri.value AS doi,
        r.title,r.publication_year AS issued_year
        FROM unnest($2::text[]) AS requested(normalized_doi)
        JOIN report_identifiers ri ON ri.scheme='doi' AND ri.normalized_value=requested.normalized_doi
        JOIN project_reports pr ON pr.project_id=$1 AND pr.report_id=ri.report_id
        JOIN reports r ON r.id=pr.report_id"#,
    )
    .bind(project_id)
    .bind(normalized_dois)
    .fetch_all(&state.pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let normalized_doi: String = row.get("normalized_doi");
            (
                normalized_doi,
                GraphReportMapping {
                    report_id: row.get("report_id"),
                    doi: row.get("doi"),
                    title: row.get("title"),
                    issued_year: row.get("issued_year"),
                },
            )
        })
        .collect())
}
