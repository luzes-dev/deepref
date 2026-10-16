//! Literature lookups for publication alerts and the `import_identifiers` block.
//!
//! The PubMed client lives in `deepref_integrations`. This module paces every PubMed request
//! through the shared PostgreSQL permit schedule, so all worker pods together stay within
//! NCBI's limit, and reports failures as block errors.

use deepref_integrations::{PubmedArticle, PubmedClient, PubmedError, REQUESTS_PER_SECOND};
use serde_json::Value;
use sqlx::PgPool;

use super::NodeError;
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
