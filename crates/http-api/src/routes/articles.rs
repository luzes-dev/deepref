use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use chrono::{DateTime, Utc};
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
        r#"SELECT r.id AS report_id, doi.value AS doi, r.title,
          r.publication_year AS issued_year, r.work_type,
          r.total_citations::int AS total_citations,
          pr.internal_citations::int AS internal_citations,
          pr.outbound_internal_references::int AS outbound_internal_references,
          pr.rank_score, pr.metrics_computed_at,
          (pr.metrics_computed_at IS NULL OR pr.metrics_computed_at < now()-interval '1 hour') AS metrics_stale
        FROM project_reports pr
        JOIN reports r ON r.id = pr.report_id
        LEFT JOIN LATERAL (
          SELECT value FROM report_identifiers
          WHERE report_id = pr.report_id AND scheme = 'doi'
          ORDER BY created_at, id LIMIT 1
        ) doi ON true
        WHERE pr.project_id = $1 AND ($2::float8 IS NULL OR
          (pr.rank_score,pr.internal_citations::int,r.total_citations::int,pr.report_id) < ($2,$3,$4,$5))
        ORDER BY pr.rank_score DESC,pr.internal_citations DESC,r.total_citations DESC,pr.report_id DESC
        LIMIT $6"#,
    )
    .bind(project_id)
    .bind(cursor.as_ref().map(|value| value.0))
    .bind(cursor.as_ref().map(|value| value.1))
    .bind(cursor.as_ref().map(|value| value.2))
    .bind(cursor.as_ref().map(|value| value.3))
    .bind(limit + 1)
    .fetch_all(&state.pool)
    .await?;
    let reports = rows.into_iter().map(report_from_row).collect();
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
        r#"SELECT r.id AS report_id,doi.value AS doi,r.title,r.abstract_text,
          r.publication_year AS issued_year,r.publication_year AS published_year,
          r.work_type,r.publisher,COALESCE(r.journal,r.container_title) AS container_title,r.url,
          r.total_citations::int AS total_citations,r.references_count::int AS references_count,
          r.raw,pr.metrics_computed_at,
          (pr.metrics_computed_at IS NULL OR pr.metrics_computed_at < now()-interval '1 hour') AS metrics_stale
        FROM project_reports pr
        JOIN reports r ON r.id = pr.report_id
        LEFT JOIN LATERAL (
          SELECT value FROM report_identifiers
          WHERE report_id = pr.report_id AND scheme = 'doi'
          ORDER BY created_at, id LIMIT 1
        ) doi ON true
        WHERE pr.project_id = $1 AND pr.report_id = $2"#,
    )
    .bind(project_id)
    .bind(report_id)
    .fetch_one(&state.pool)
    .await?;
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
        metrics_stale: row.get("metrics_stale"),
        raw: row.get("raw"),
    }))
}

#[utoipa::path(get, path="/projects/{project_id}/graph", operation_id="getProjectGraph", tag="reports",
    params(("project_id"=Uuid, Path)),
    responses((status=200,body=ProjectGraphDto),(status=500,body=ErrorResponse)))]
pub(crate) async fn project_graph(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<ProjectGraphDto>, ApiError> {
    let projection = projection_metadata(&state, project_id).await?;
    let graph = deepref_postgres::load_project_graph(&state.pool, project_id).await?;
    let nodes = graph
        .nodes
        .into_iter()
        .map(report_from_graph_node)
        .collect();
    let edges = graph
        .edges
        .into_iter()
        .map(|edge| GraphEdgeDto {
            source: edge.source,
            target: edge.target,
        })
        .collect();
    Ok(Json(ProjectGraphDto {
        nodes,
        edges,
        projection,
        truncated: graph.truncated,
    }))
}

#[utoipa::path(get, path="/projects/{project_id}/recommendations", operation_id="getProjectRecommendations", tag="reports",
    params(("project_id"=Uuid, Path)), responses((status=200,body=RecommendationGroupsDto),(status=500,body=ErrorResponse)))]
pub(crate) async fn recommendations(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<RecommendationGroupsDto>, ApiError> {
    let graph = project_graph(State(state), Path(project_id)).await?.0;
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
        "INSERT INTO domain_events (event_id,schema_version,event_type,entity_type,entity_key,revision,payload,correlation_id,causation_id,created_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(event.event_id)
    .bind(event.schema_version as i16)
    .bind(&event.event_type)
    .bind(event.entity_type.as_str())
    .bind(&event.entity_key)
    .bind(event.revision)
    .bind(serde_json::to_value(&event.payload)?)
    .bind(event.correlation_id)
    .bind(event.causation_id)
    .bind(event.occurred_at)
    .execute(&mut *tx)
    .await?;
    deepref_postgres::enqueue_job(
        &mut tx,
        &deepref_postgres::job(
            event.event_id,
            "recompute_metrics",
            serde_json::to_value(&event)?,
            format!("recompute_metrics:{project_id}:{}", event.event_id),
        ),
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
    "SELECT revision,lag,last_success_at FROM projection_state WHERE projection_name='postgres_graph' AND (project_id=$1 OR project_id IS NULL) ORDER BY project_id NULLS LAST LIMIT 1",
    )
    .bind(project_id)
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.map_or(
        ProjectionMetadata {
            revision: 0,
            lag: 0,
            last_success_at: None,
        },
        |row| ProjectionMetadata {
            revision: row.get("revision"),
            lag: row.get("lag"),
            last_success_at: row.get("last_success_at"),
        },
    ))
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

fn report_from_graph_node(node: deepref_graph::GraphNode) -> ReportDto {
    ReportDto {
        report_id: node.report_id,
        doi: node.doi,
        title: node.title,
        issued_year: node.issued_year,
        work_type: node.work_type,
        total_citations: node.total_citations as i32,
        internal_citations: node.internal_citations as i32,
        outbound_internal_references: node.outbound_internal_references as i32,
        rank_score: node.rank_score,
        metrics_as_of: node.metrics_as_of,
        metrics_stale: node.metrics_as_of.is_none(),
    }
}
