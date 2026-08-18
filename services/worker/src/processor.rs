use std::time::Duration;

use deepref_core::{IngestionItemStatus, normalize_doi};
use deepref_crossref::{CrossrefClient, CrossrefError};
use deepref_events::{DeadLetterRecord, EventEnvelope, WorkFetchRequested, deserialize_compatible};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::sync::watch;
use uuid::Uuid;

use crate::{
    delivery::{DeliveryAction, FailureClass, action_for},
    limiter, store,
};

pub async fn handle_message(
    pool: PgPool,
    bytes: Vec<u8>,
    delivery_count: u64,
    claim_lease: Duration,
) -> anyhow::Result<DeliveryAction> {
    let event: EventEnvelope<WorkFetchRequested> = match deserialize_compatible(&bytes) {
        Ok(event) => event,
        Err(error) => {
            let record = dead_letter(&bytes, None, delivery_count, "MALFORMED_PAYLOAD");
            store::persist_malformed_dead_letter(
                &pool,
                &record,
                serde_json::json!({ "payload_utf8_lossy": String::from_utf8_lossy(&bytes), "error": error.to_string() }),
            ).await?;
            return Ok(action_for(FailureClass::Malformed, delivery_count));
        }
    };
    let owner = Uuid::new_v4();
    match store::claim_event(&pool, event.event_id, owner, claim_lease).await? {
        store::ClaimState::Completed => return Ok(DeliveryAction::Ack),
        store::ClaimState::Busy => {
            return Ok(DeliveryAction::Nak(
                (claim_lease / 3).max(Duration::from_secs(1)),
            ));
        }
        store::ClaimState::Acquired => {}
    }

    let doi = match normalize_doi(&event.payload.doi) {
        Ok(doi) => doi,
        Err(error) => {
            let record = dead_letter(&bytes, Some(event.event_id), delivery_count, "INVALID_DOI");
            store::complete_without_doi(
                &pool,
                &event,
                &event.payload.doi,
                owner,
                IngestionItemStatus::Failed,
                Some(&error.to_string()),
                Some(&record),
            )
            .await?;
            return Ok(DeliveryAction::Terminate);
        }
    };
    store::mark_fetching(&pool, &event, &doi).await?;
    if event.payload.depth > event.payload.max_depth {
        store::complete_without_doi(
            &pool,
            &event,
            &doi,
            owner,
            IngestionItemStatus::Skipped,
            Some("maximum depth exceeded"),
            None,
        )
        .await?;
        return Ok(DeliveryAction::Ack);
    }
    if store::ingestion_cancelled(&pool, event.payload.ingestion_id).await? {
        store::complete_without_doi(
            &pool,
            &event,
            &doi,
            owner,
            IngestionItemStatus::Skipped,
            Some("ingestion cancelled"),
            None,
        )
        .await?;
        return Ok(DeliveryAction::Ack);
    }
    if store::is_cached(&pool, &doi).await? {
        store::attach_cached(&pool, &event, &doi, owner).await?;
        return Ok(DeliveryAction::Ack);
    }
    if store::claim_doi(&pool, &doi, owner, claim_lease).await? != store::ClaimState::Acquired {
        let action = action_for(FailureClass::Retryable, delivery_count);
        if action == DeliveryAction::Terminate {
            let record = dead_letter(
                &bytes,
                Some(event.event_id),
                delivery_count,
                "DOI_LEASE_EXHAUSTED",
            );
            store::complete_without_doi(
                &pool,
                &event,
                &doi,
                owner,
                IngestionItemStatus::Failed,
                Some("DOI lease remained busy through the final delivery"),
                Some(&record),
            )
            .await?;
        } else {
            store::release_event_claim(&pool, event.event_id, owner, "DOI lease is busy").await?;
        }
        return Ok(action);
    }

    let settings = store::load_runtime_settings(&pool).await?;
    let client = match CrossrefClient::new(settings.crossref_mailto.clone()) {
        Ok(client) => client.with_max_attempts(settings.retry_attempts),
        Err(error) => {
            store::finalize_terminal_failure(
                &pool,
                &event,
                &doi,
                owner,
                IngestionItemStatus::Failed,
                &error.to_string(),
                None,
            )
            .await?;
            return Ok(DeliveryAction::Ack);
        }
    };
    limiter::acquire(&pool, "crossref", settings.rate_limit_per_second).await?;
    let (cancel_heartbeat, heartbeat_done) = spawn_heartbeat(
        pool.clone(),
        event.event_id,
        doi.clone(),
        owner,
        claim_lease,
    );
    let fetched = client.fetch_work(&doi).await;
    cancel_heartbeat.send_replace(true);
    let _ = heartbeat_done.await;

    match fetched {
        Ok(work) => {
            store::finalize_success(&pool, &event, &doi, owner, &work).await?;
            Ok(DeliveryAction::Ack)
        }
        Err(error) if is_retryable_crossref_error(&error) => {
            let action = action_for(FailureClass::Retryable, delivery_count);
            if action == DeliveryAction::Terminate {
                let record = dead_letter(
                    &bytes,
                    Some(event.event_id),
                    delivery_count,
                    "DELIVERY_EXHAUSTED",
                );
                store::finalize_terminal_failure(
                    &pool,
                    &event,
                    &doi,
                    owner,
                    IngestionItemStatus::Failed,
                    &error.to_string(),
                    Some(&record),
                )
                .await?;
            } else {
                store::release_retryable(&pool, event.event_id, &doi, owner, &error.to_string())
                    .await?;
            }
            Ok(action)
        }
        Err(error) => {
            let status = if matches!(error, CrossrefError::NotFound(_)) {
                IngestionItemStatus::NotFound
            } else {
                IngestionItemStatus::Failed
            };
            store::finalize_terminal_failure(
                &pool,
                &event,
                &doi,
                owner,
                status,
                &error.to_string(),
                None,
            )
            .await?;
            Ok(DeliveryAction::Ack)
        }
    }
}

fn spawn_heartbeat(
    pool: PgPool,
    event_id: Uuid,
    doi: String,
    owner: Uuid,
    lease: Duration,
) -> (watch::Sender<bool>, tokio::task::JoinHandle<()>) {
    let (sender, mut receiver) = watch::channel(false);
    let interval = (lease / 3).max(Duration::from_secs(1));
    let handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                result = receiver.changed() => {
                    if result.is_err() || *receiver.borrow() { break; }
                }
                _ = tokio::time::sleep(interval) => {
                    match store::renew_claims(&pool, event_id, &doi, owner, lease).await {
                        Ok(true) => tracing::debug!(%event_id, %doi, "processing lease renewed"),
                        Ok(false) => { tracing::error!(%event_id, %doi, "processing lease was lost"); break; }
                        Err(error) => tracing::warn!(%error, %event_id, %doi, "lease heartbeat failed"),
                    }
                }
            }
        }
    });
    (sender, handle)
}

fn dead_letter(
    bytes: &[u8],
    event_id: Option<Uuid>,
    delivery_count: u64,
    reason: &str,
) -> DeadLetterRecord {
    let digest = Sha256::digest(bytes);
    let payload_sha256 = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    DeadLetterRecord {
        identity: format!("sha256:{payload_sha256}"),
        source_subject: deepref_events::SUBJECT_WORK_FETCH_REQUESTED.to_owned(),
        source_event_id: event_id,
        delivery_count,
        reason_code: reason.to_owned(),
        payload_sha256,
    }
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
    #[test]
    fn malformed_identity_is_stable() {
        assert_eq!(
            dead_letter(b"bad", None, 1, "bad").identity,
            dead_letter(b"bad", None, 5, "other").identity
        );
    }
}
