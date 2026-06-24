use axum::{
    Json,
    extract::{Path, State},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Serialize;
use sqlx::Row;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    error::{ApiError, ErrorResponse},
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
    #[schema(value_type = Object)]
    raw: serde_json::Value,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct GraphEdgeDto {
    source: String,
    target: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ProjectGraphDto {
    nodes: Vec<ArticleDto>,
    edges: Vec<GraphEdgeDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RecommendationGroupsDto {
    foundational: Vec<ArticleDto>,
    core_to_project: Vec<ArticleDto>,
    underexplored: Vec<ArticleDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RecomputeMetricsDto {
    status: &'static str,
    project_id: Uuid,
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/articles",
    operation_id = "listProjectArticles",
    tag = "articles",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    responses(
        (status = 200, description = "Project articles ranked by citation metrics", body = [ArticleDto]),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn list_articles(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<Vec<ArticleDto>>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT w.canonical_doi, w.title, w.issued_year, w.work_type, w.total_citations,
               COALESCE(pi.internal_citations, 0) AS internal_citations,
               COALESCE(pi.outbound_internal_references, 0) AS outbound_internal_references,
               COALESCE(pi.rank_score, 0) AS rank_score
        FROM project_works pi
        JOIN works w ON w.canonical_doi = pi.canonical_doi
        WHERE pi.project_id = $1
        ORDER BY pi.rank_score DESC, pi.internal_citations DESC, w.total_citations DESC
        "#,
    )
    .bind(project_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows.into_iter().map(article_row_json).collect()))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/articles/{doi_key}",
    operation_id = "getProjectArticle",
    tag = "articles",
    params(
        ("project_id" = Uuid, Path, description = "Project identifier"),
        ("doi_key" = String, Path, description = "Base64url-encoded canonical DOI")
    ),
    responses(
        (status = 200, description = "Project article metadata", body = ArticleDetailDto),
        (status = 400, description = "Invalid DOI key", body = ErrorResponse),
        (status = 404, description = "Article not found", body = ErrorResponse),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn get_article(
    State(state): State<AppState>,
    Path((project_id, doi_key)): Path<(Uuid, String)>,
) -> Result<Json<ArticleDetailDto>, ApiError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(doi_key.as_bytes())
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let doi = String::from_utf8(bytes).map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let row = sqlx::query(
        r#"
        SELECT canonical_doi, title, abstract_text, issued_year, published_year, work_type,
               publisher, container_title, url, total_citations, references_count, raw
        FROM works
        WHERE canonical_doi = $1
          AND EXISTS (
            SELECT 1 FROM project_works
            WHERE project_id = $2 AND canonical_doi = works.canonical_doi
          )
        "#,
    )
    .bind(doi)
    .bind(project_id)
    .fetch_one(&state.pool)
    .await?;
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
        raw: row.get("raw"),
    }))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/graph",
    operation_id = "getProjectGraph",
    tag = "articles",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    responses(
        (status = 200, description = "Project citation graph", body = ProjectGraphDto),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn project_graph(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<ProjectGraphDto>, ApiError> {
    let nodes = list_articles(State(state.clone()), Path(project_id))
        .await?
        .0;
    let rows = sqlx::query(
        r#"
        SELECT source_doi, target_doi FROM citations
        WHERE project_id = $1
        "#,
    )
    .bind(project_id)
    .fetch_all(&state.pool)
    .await?;
    let edges = rows
        .into_iter()
        .map(|row| GraphEdgeDto {
            source: row.get("source_doi"),
            target: row.get("target_doi"),
        })
        .collect();
    Ok(Json(ProjectGraphDto { nodes, edges }))
}

#[utoipa::path(
    get,
    path = "/projects/{project_id}/recommendations",
    operation_id = "getProjectRecommendations",
    tag = "articles",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    responses(
        (status = 200, description = "Project recommendation groups", body = RecommendationGroupsDto),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn recommendations(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<RecommendationGroupsDto>, ApiError> {
    let articles = list_articles(State(state), Path(project_id)).await?.0;
    Ok(Json(RecommendationGroupsDto {
        foundational: articles.iter().take(5).cloned().collect(),
        core_to_project: articles.iter().take(5).cloned().collect(),
        underexplored: articles.iter().rev().take(5).cloned().collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/projects/{project_id}/metrics/recompute",
    operation_id = "recomputeProjectMetrics",
    tag = "articles",
    params(("project_id" = Uuid, Path, description = "Project identifier")),
    responses(
        (status = 200, description = "Metrics recomputation queued", body = RecomputeMetricsDto),
        (status = 500, description = "Internal server error", body = ErrorResponse)
    )
)]
pub(crate) async fn recompute_metrics(
    State(state): State<AppState>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<RecomputeMetricsDto>, ApiError> {
    sqlx::query("SELECT recompute_project_metrics($1)")
        .bind(project_id)
        .execute(&state.pool)
        .await?;
    Ok(Json(RecomputeMetricsDto {
        status: "queued",
        project_id,
    }))
}

fn article_row_json(row: sqlx::postgres::PgRow) -> ArticleDto {
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
    }
}
