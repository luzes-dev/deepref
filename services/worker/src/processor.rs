use deepref_core::{FetchStatus, IngestionItemStatus, normalize_doi};
use deepref_crossref::{CrossrefClient, CrossrefError};
use deepref_events::{EventEnvelope, WorkFetchRequested};
use sqlx::PgPool;
use std::sync::Arc;

use crate::{limiter::DirectRateLimiter, nats, store};

pub(crate) async fn handle_message(
    pool: PgPool,
    limiter: Arc<DirectRateLimiter>,
    bytes: Vec<u8>,
) -> anyhow::Result<()> {
    let event: EventEnvelope<WorkFetchRequested> = serde_json::from_slice(&bytes)?;
    if store::event_processed(&pool, event.id).await? {
        tracing::debug!(event_id = %event.id, "event already processed");
        return Ok(());
    }

    let payload = event.payload.clone();
    let doi = normalize_doi(&payload.doi)?;

    if payload.depth > payload.max_depth {
        store::mark_item(&pool, &payload, &doi, IngestionItemStatus::Skipped, None).await?;
        store::mark_event_processed(&pool, event.id).await?;
        return Ok(());
    }

    if !store::claim_item(&pool, &payload, &doi).await? {
        tracing::debug!(doi, "item already terminal or claimed");
        store::mark_event_processed(&pool, event.id).await?;
        return Ok(());
    }

    if store::ingestion_cancelled(&pool, payload.ingestion_id).await? {
        store::mark_item(
            &pool,
            &payload,
            &doi,
            IngestionItemStatus::Skipped,
            Some("ingestion cancelled"),
        )
        .await?;
        store::mark_event_processed(&pool, event.id).await?;
        return Ok(());
    }

    if !store::claim_global_fetch(&pool, &doi).await? {
        store::mark_item(
            &pool,
            &payload,
            &doi,
            IngestionItemStatus::Skipped,
            Some("already fetched or in progress"),
        )
        .await?;
        store::mark_event_processed(&pool, event.id).await?;
        return Ok(());
    }

    let settings = store::load_runtime_settings(&pool).await?;
    let client =
        CrossrefClient::new(settings.crossref_mailto)?.with_max_attempts(settings.retry_attempts);
    limiter.until_ready().await;

    match client.fetch_work(&doi).await {
        Ok(work) => {
            store::persist_work(&pool, &payload, &work).await?;
            store::mark_item(&pool, &payload, &doi, IngestionItemStatus::Fetched, None).await?;
            store::mark_global_fetch(&pool, &doi, FetchStatus::Fetched, None).await?;

            let mut discovered = 0usize;
            for reference in &work.references {
                let Some(reference_doi) = &reference.doi else {
                    store::persist_unresolved_reference(&pool, &payload, &doi, reference).await?;
                    continue;
                };
                store::persist_citation(&pool, &payload, &doi, reference_doi).await?;
                if payload.depth < payload.max_depth {
                    discovered += 1;
                    nats::enqueue_reference(&pool, &event, &payload, reference_doi).await?;
                }
            }

            nats::publish_completed(&pool, &payload, &doi, discovered).await?;
            nats::publish_metrics_recompute(&pool, &payload).await?;
        }
        Err(error) => {
            let retryable = is_retryable_crossref_error(&error);
            let item_status = if matches!(error, CrossrefError::NotFound(_)) {
                IngestionItemStatus::NotFound
            } else {
                IngestionItemStatus::Failed
            };
            let fetch_status = if matches!(error, CrossrefError::NotFound(_)) {
                FetchStatus::NotFound
            } else {
                FetchStatus::Failed
            };
            let message = error.to_string();
            store::mark_item(&pool, &payload, &doi, item_status, Some(&message)).await?;
            store::mark_global_fetch(&pool, &doi, fetch_status, Some(&message)).await?;
            nats::publish_failed(&pool, &payload, &doi, &message, retryable).await?;
        }
    }

    store::mark_event_processed(&pool, event.id).await?;
    Ok(())
}

fn is_retryable_crossref_error(error: &CrossrefError) -> bool {
    matches!(
        error,
        CrossrefError::RetryableStatus(_) | CrossrefError::Request(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn classifies_retryable_crossref_errors() {
        assert!(is_retryable_crossref_error(
            &CrossrefError::RetryableStatus(StatusCode::TOO_MANY_REQUESTS)
        ));
        assert!(!is_retryable_crossref_error(&CrossrefError::NotFound(
            "10.1/x".to_owned()
        )));
    }
}
