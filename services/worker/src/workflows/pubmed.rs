//! Literature lookups for publication alerts and the `import_identifiers` block.
//!
//! The PubMed client lives in `deepref_providers`. This module paces every PubMed request
//! through the shared PostgreSQL permit schedule, so all worker pods together stay within
//! NCBI's limit, and reports failures as block errors. Crossref searches stay here.

use deepref_providers::{PubmedArticle, PubmedClient, PubmedError, REQUESTS_PER_SECOND};
use serde_json::{Value, json};
use sqlx::PgPool;

use super::{NodeError, net};
use crate::limiter;

/// PubMed's entry in the shared provider schedule.
const PUBMED_PROVIDER: &str = "pubmed";

fn node_error(error: PubmedError) -> NodeError {
    match error {
        PubmedError::Retryable(message) => NodeError::retryable(message),
        PubmedError::Permanent(message) => NodeError::permanent(message),
    }
}

/// Waits for this request's turn with PubMed, shared with every other worker.
pub(crate) async fn pace(pool: &PgPool) -> Result<(), NodeError> {
    limiter::acquire(pool, PUBMED_PROVIDER, REQUESTS_PER_SECOND)
        .await
        .map_err(|_| NodeError::retryable("The database was busy. This will be tried again."))
}

/// Fetch articles for PubMed IDs.
pub async fn fetch_pubmed(
    pool: &PgPool,
    client: &PubmedClient,
    pmids: &[String],
) -> Result<Vec<PubmedArticle>, NodeError> {
    if pmids.is_empty() {
        return Ok(Vec::new());
    }
    pace(pool).await?;
    client.fetch(pmids).await.map_err(node_error)
}

/// Search PubMed for records indexed in the last `days` days.
pub async fn search_pubmed(
    pool: &PgPool,
    client: &PubmedClient,
    terms: &str,
    days: u32,
) -> Result<Vec<Value>, NodeError> {
    pace(pool).await?;
    let ids = client.search(terms, days).await.map_err(node_error)?;
    let articles = fetch_pubmed(pool, client, &ids).await?;
    Ok(articles.iter().map(PubmedArticle::to_item).collect())
}

fn encode(text: &str) -> String {
    reqwest::Url::parse_with_params("https://x.invalid/", [("q", text)])
        .map(|url| {
            url.query()
                .unwrap_or("q=")
                .trim_start_matches("q=")
                .to_owned()
        })
        .unwrap_or_default()
}
/// Search Crossref for works indexed since `from_date` (`YYYY-MM-DD`).
pub async fn search_crossref(
    client: &reqwest::Client,
    terms: &str,
    from_date: &str,
    mailto: &str,
) -> Result<Vec<Value>, NodeError> {
    let mut url = format!(
        "https://api.crossref.org/works?query.bibliographic={}&filter=from-index-date:{}&rows=50&sort=indexed&order=desc&select=DOI,title,container-title,issued,author,abstract",
        encode(terms),
        encode(from_date)
    );
    if !mailto.trim().is_empty() {
        url.push_str(&format!("&mailto={}", encode(mailto.trim())));
    }
    let body = net::trusted_get(client, &url).await?;
    let parsed: Value = serde_json::from_str(&body).map_err(|_| {
        NodeError::retryable("Crossref sent an answer that could not be understood.")
    })?;
    let items = parsed
        .pointer("/message/items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(items
        .iter()
        .map(|work| {
            let first = |key: &str| {
                work.get(key)
                    .and_then(Value::as_array)
                    .and_then(|list| list.first())
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            };
            let authors: Vec<String> = work
                .get("author")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .map(|author| {
                            format!(
                                "{} {}",
                                author
                                    .get("given")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default(),
                                author
                                    .get("family")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                            )
                            .trim()
                            .to_owned()
                        })
                        .filter(|name| !name.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            json!({
                "source": "crossref",
                "doi": work.get("DOI").and_then(Value::as_str).map(str::to_lowercase),
                "pmid": Value::Null,
                "title": first("title"),
                "abstract": work.get("abstract").and_then(Value::as_str),
                "journal": first("container-title"),
                "year": work.pointer("/issued/date-parts/0/0").and_then(Value::as_i64),
                "authors": authors,
            })
        })
        .collect())
}
