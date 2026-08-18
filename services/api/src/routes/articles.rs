use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
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
    outbox,
    state::AppState,
};

#[derive(Debug, Serialize, Clone, ToSchema)]
pub(crate) struct ArticleDto {
    doi: String,
    doi_key: String,
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
pub(crate) struct ArticleDetailDto {
    doi: String,
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
    source: String,
    target: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ProjectionMetadata {
    revision: i64,
    lag: i64,
    last_success_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ProjectGraphDto {
    nodes: Vec<ArticleDto>,
    edges: Vec<GraphEdgeDto>,
    projection: ProjectionMetadata,
    truncated: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RecommendationGroupsDto {
    foundational: Vec<ArticleDto>,
    core_to_project: Vec<ArticleDto>,
    underexplored: Vec<ArticleDto>,
    projection: ProjectionMetadata,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RecomputeMetricsDto {
    status: &'static str,
    project_id: Uuid,
    event_id: Uuid,
}

#[utoipa::path(get, path="/projects/{project_id}/articles", operation_id="listProjectArticles", tag="articles",
    params(("project_id"=Uuid, Path), PaginationParams),
    responses((status=200, body=PaginatedResponse<ArticleDto>), (status=500, body=ErrorResponse)))]
pub(crate) async fn list_articles(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
    Query(pagination): Query<PaginationParams>,
) -> Result<Json<PaginatedResponse<ArticleDto>>, ApiError> {
    let limit = pagination.limit()?;
    let cursor: Option<(f64, i32, i32, String)> = pagination.decode()?;
    let rows = sqlx::query(
        r#"SELECT w.canonical_doi,w.title,w.issued_year,w.work_type,w.total_citations,
        COALESCE(pw.internal_citations,0) AS internal_citations,
        COALESCE(pw.outbound_internal_references,0) AS outbound_internal_references,
        COALESCE(pw.rank_score,0) AS rank_score,pw.metrics_computed_at,
        (pw.metrics_computed_at IS NULL OR pw.metrics_computed_at < now()-interval '1 hour') AS metrics_stale
        FROM project_works pw JOIN works w ON w.canonical_doi=pw.canonical_doi
        WHERE pw.project_id=$1 AND ($2::float8 IS NULL OR
          (pw.rank_score,pw.internal_citations,w.total_citations,w.canonical_doi)<($2,$3,$4,$5))
        ORDER BY pw.rank_score DESC,pw.internal_citations DESC,w.total_citations DESC,w.canonical_doi DESC LIMIT $6"#,
    ).bind(project_id).bind(cursor.as_ref().map(|value| value.0))
        .bind(cursor.as_ref().map(|value| value.1)).bind(cursor.as_ref().map(|value| value.2))
        .bind(cursor.as_ref().map(|value| value.3.clone())).bind(limit + 1)
        .fetch_all(&state.pool).await?;
    let graph_stale = match &state.graph {
        Some(graph) => graph.ping().await.is_err(),
        None => true,
    };
    let articles = rows
        .into_iter()
        .map(article_from_row)
        .map(|mut article| {
            article.metrics_stale |= graph_stale;
            article
        })
        .collect();
    Ok(Json(page(articles, limit as usize, |article| {
        (
            article.rank_score,
            article.internal_citations,
            article.total_citations,
            article.doi.clone(),
        )
    })?))
}

#[utoipa::path(get, path="/projects/{project_id}/articles/{doi_key}", operation_id="getProjectArticle", tag="articles",
    params(("project_id"=Uuid, Path),("doi_key"=String, Path)),
    responses((status=200, body=ArticleDetailDto),(status=400,body=ErrorResponse),(status=404,body=ErrorResponse)))]
pub(crate) async fn get_article(
    State(state): State<AppState>,
    Path((project_id, doi_key)): Path<(Uuid, String)>,
) -> Result<Json<ArticleDetailDto>, ApiError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(doi_key.as_bytes())
        .map_err(|_| ApiError::BadRequest("invalid DOI key".into()))?;
    let doi =
        String::from_utf8(bytes).map_err(|_| ApiError::BadRequest("invalid DOI key".into()))?;
    let row = sqlx::query(
        r#"SELECT w.canonical_doi,w.title,w.abstract_text,w.issued_year,w.published_year,w.work_type,
        w.publisher,w.container_title,w.url,w.total_citations,w.references_count,w.raw,
        pw.metrics_computed_at,(pw.metrics_computed_at IS NULL OR pw.metrics_computed_at<now()-interval '1 hour') AS metrics_stale
        FROM works w JOIN project_works pw ON pw.canonical_doi=w.canonical_doi
        WHERE w.canonical_doi=$1 AND pw.project_id=$2"#,
    ).bind(doi).bind(project_id).fetch_one(&state.pool).await?;
    let graph_stale = match &state.graph {
        Some(graph) => graph.ping().await.is_err(),
        None => true,
    };
    Ok(Json(ArticleDetailDto {
        doi: row.get("canonical_doi"),
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

#[utoipa::path(get, path="/projects/{project_id}/graph", operation_id="getProjectGraph", tag="articles",
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
    let nodes = graph_data
        .nodes
        .into_iter()
        .map(|node| ArticleDto {
            doi_key: URL_SAFE_NO_PAD.encode(node.doi.as_bytes()),
            doi: node.doi,
            title: node.title,
            issued_year: node.issued_year.map(|year| year as i32),
            work_type: None,
            total_citations: node.total_citations as i32,
            internal_citations: 0,
            outbound_internal_references: 0,
            rank_score: 0.0,
            metrics_as_of: projection.last_success_at,
            metrics_stale: projection.lag > 0,
        })
        .collect();
    let edges = graph_data
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
        truncated: graph_data.truncated,
    }))
}

#[utoipa::path(get, path="/projects/{project_id}/recommendations", operation_id="getProjectRecommendations", tag="articles",
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

#[utoipa::path(post, path="/projects/{project_id}/metrics/recompute", operation_id="recomputeProjectMetrics", tag="articles",
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

fn article_from_row(row: sqlx::postgres::PgRow) -> ArticleDto {
    let doi: String = row.get("canonical_doi");
    ArticleDto {
        doi_key: URL_SAFE_NO_PAD.encode(doi.as_bytes()),
        doi,
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
