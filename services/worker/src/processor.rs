use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

use deepref_core::{IngestionItemStatus, normalize_doi};
use deepref_crossref::CrossrefError;
use deepref_documents::{
    DocumentParser, DocumentStore, HttpsPdfFetcher, PARSER_VERSION, PdfiumParser,
    RemoteDocumentFetcher,
};
use deepref_events::{DeadLetterRecord, EventEnvelope, WorkFetchRequested, deserialize_compatible};
use deepref_providers::CrossrefProvider;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::sync::{Semaphore, watch};
use uuid::Uuid;

use crate::{
    delivery::{DeliveryAction, FailureClass, action_for},
    limiter, store,
};

static PDF_PARSE_SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();

pub fn validate_pdf_parse_concurrency() -> anyhow::Result<()> {
    if let Ok(value) = std::env::var("DOCUMENT_PARSE_CONCURRENCY") {
        let parsed = value.parse::<usize>().map_err(|_| {
            anyhow::anyhow!("DOCUMENT_PARSE_CONCURRENCY must be a positive integer up to 16")
        })?;
        if !(1..=16).contains(&parsed) {
            anyhow::bail!("DOCUMENT_PARSE_CONCURRENCY must be a positive integer up to 16");
        }
    }
    Ok(())
}

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
    let client = match CrossrefProvider::new(settings.crossref_mailto.clone()) {
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
    let fetched = client.fetch_work_with_references(&doi).await;
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

pub async fn handle_job(
    pool: sqlx::PgPool,
    job: &deepref_application::jobs::ClaimedJob,
    claim_lease: Duration,
) -> anyhow::Result<DeliveryAction> {
    handle_job_with_document_services(pool, job, claim_lease, None, None, None).await
}

pub async fn handle_job_with_documents(
    pool: sqlx::PgPool,
    job: &deepref_application::jobs::ClaimedJob,
    claim_lease: Duration,
    document_store: Option<Arc<DocumentStore>>,
    document_parser: Option<Arc<dyn DocumentParser>>,
) -> anyhow::Result<DeliveryAction> {
    handle_job_with_document_services(
        pool,
        job,
        claim_lease,
        document_store,
        document_parser,
        None,
    )
    .await
}

pub async fn handle_job_with_document_services(
    pool: sqlx::PgPool,
    job: &deepref_application::jobs::ClaimedJob,
    claim_lease: Duration,
    document_store: Option<Arc<DocumentStore>>,
    document_parser: Option<Arc<dyn DocumentParser>>,
    remote_fetcher: Option<Arc<dyn RemoteDocumentFetcher>>,
) -> anyhow::Result<DeliveryAction> {
    match job.kind.as_str() {
        "work_fetch_requested" => {
            let bytes = serde_json::to_vec(&job.payload)?;
            handle_message(pool, bytes, job.attempts.max(1) as u64, claim_lease).await
        }
        "recompute_metrics" => {
            let event: EventEnvelope<deepref_events::DomainPayload> =
                serde_json::from_value(job.payload.clone())?;
            let project_id = match event.payload {
                deepref_events::DomainPayload::MetricsRecomputeRequested(payload) => {
                    payload.project_id
                }
                _ => anyhow::bail!("recompute_metrics job has an unsupported payload"),
            };
            deepref_postgres::recompute_project_metrics(&pool, project_id).await?;
            Ok(DeliveryAction::Ack)
        }
        "recompute_prisma" => {
            let project_id = job
                .payload
                .get("project_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("recompute_prisma payload is missing project_id"))?
                .parse()?;
            deepref_postgres::recompute_prisma_snapshot(&pool, project_id).await?;
            Ok(DeliveryAction::Ack)
        }
        "retrieve_document" => {
            let document_id = job
                .payload
                .get("document_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("retrieve_document payload is missing document_id"))?
                .parse::<Uuid>()?;
            let document = deepref_postgres::get_document_by_id(&pool, document_id).await?;
            if document.object_key.is_some()
                && matches!(document.status.as_str(), "uploaded" | "available")
            {
                return Ok(DeliveryAction::Ack);
            }
            let external_url = document
                .external_url
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("external document has no source URL"))?;
            let store = match document_store {
                Some(store) => store,
                None => Arc::new(DocumentStore::from_env()?),
            };
            if !deepref_postgres::mark_document_retrieving(&pool, document_id).await? {
                return Ok(DeliveryAction::Ack);
            }
            let fetcher: Arc<dyn RemoteDocumentFetcher> =
                remote_fetcher.unwrap_or_else(|| Arc::new(HttpsPdfFetcher::default()));
            let (stored, _) = match fetcher.fetch(external_url, &store).await {
                Ok(result) => result,
                Err(error) => {
                    deepref_postgres::mark_document_retrieval_failed(
                        &pool,
                        document_id,
                        &error.to_string(),
                    )
                    .await?;
                    return Err(error.into());
                }
            };
            let byte_size = match i64::try_from(stored.byte_size) {
                Ok(value) => value,
                Err(error) => {
                    let _ = store.delete(&stored.opaque_id).await;
                    deepref_postgres::mark_document_retrieval_failed(
                        &pool,
                        document_id,
                        "retrieved document size overflowed persistence bounds",
                    )
                    .await?;
                    return Err(error.into());
                }
            };
            let mut transaction = pool.begin().await?;
            let completion = match deepref_postgres::complete_document_retrieval(
                &mut transaction,
                document_id,
                &stored.opaque_id,
                &stored.sha256,
                byte_size,
            )
            .await
            {
                Ok(outcome) => outcome,
                Err(error) => {
                    let _ = transaction.rollback().await;
                    let _ = store.delete(&stored.opaque_id).await;
                    deepref_postgres::mark_document_retrieval_failed(
                        &pool,
                        document_id,
                        "retrieved document could not be persisted",
                    )
                    .await?;
                    return Err(error);
                }
            };
            if let Err(error) = transaction.commit().await {
                let _ = store.delete(&stored.opaque_id).await;
                if matches!(
                    completion,
                    deepref_postgres::CompleteDocumentRetrievalOutcome::Applied
                ) {
                    deepref_postgres::mark_document_retrieval_failed(
                        &pool,
                        document_id,
                        "retrieved document transaction could not commit",
                    )
                    .await?;
                }
                return Err(error.into());
            }
            match completion {
                deepref_postgres::CompleteDocumentRetrievalOutcome::Applied => {}
                deepref_postgres::CompleteDocumentRetrievalOutcome::AlreadyCompleted => {
                    store.delete(&stored.opaque_id).await?;
                }
            }
            Ok(DeliveryAction::Ack)
        }
        "parse_document" => {
            let document_id = job
                .payload
                .get("document_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("parse_document payload is missing document_id"))?
                .parse::<Uuid>()?;
            let requested_version = job
                .payload
                .get("parser_version")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(PARSER_VERSION);
            if requested_version != PARSER_VERSION {
                anyhow::bail!("parse_document requested unsupported parser version");
            }
            let document = deepref_postgres::get_document_by_id(&pool, document_id).await?;
            if document.active_parser_version.as_deref() == Some(PARSER_VERSION) {
                return Ok(DeliveryAction::Ack);
            }
            let object_key = document
                .object_key
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("document has no stored object"))?;
            let store = match document_store {
                Some(store) => store,
                None => Arc::new(DocumentStore::from_env()?),
            };
            deepref_postgres::mark_document_parsing(&pool, document_id).await?;
            let parse_permit = pdf_parse_semaphore()
                .acquire_owned()
                .await
                .map_err(|error| {
                    anyhow::anyhow!("PDF parser concurrency limiter closed: {error}")
                })?;
            let parsed = match parse_stored_document(
                &document,
                object_key,
                Arc::clone(&store),
                document_parser,
            )
            .await
            {
                Ok(parsed) => parsed,
                Err(error) => {
                    drop(parse_permit);
                    deepref_postgres::mark_document_failed(&pool, document_id, &error.to_string())
                        .await?;
                    return Err(error);
                }
            };
            drop(parse_permit);
            deepref_postgres::persist_parsed_document(&pool, document_id, &parsed, PARSER_VERSION)
                .await?;
            Ok(DeliveryAction::Ack)
        }
        other => anyhow::bail!("unsupported durable job kind: {other}"),
    }
}

fn pdf_parse_semaphore() -> Arc<Semaphore> {
    Arc::clone(PDF_PARSE_SEMAPHORE.get_or_init(|| {
        let permits = std::env::var("DOCUMENT_PARSE_CONCURRENCY")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| (1..=16).contains(value))
            .unwrap_or(2);
        Arc::new(Semaphore::new(permits))
    }))
}

async fn parse_stored_document(
    document: &deepref_postgres::DocumentRecord,
    object_key: &str,
    store: Arc<DocumentStore>,
    parser: Option<Arc<dyn DocumentParser>>,
) -> anyhow::Result<deepref_documents::ParsedDocument> {
    let path = std::env::temp_dir().join(format!("deepref-parse-{}.pdf", Uuid::new_v4()));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await?;
    let read = store.read_to_writer(object_key, &mut file).await;
    drop(file);
    let stored = match read {
        Ok(stored) => stored,
        Err(error) => {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(error.into());
        }
    };
    if document.content_hash.as_deref() != Some(stored.sha256.as_str())
        || usize::try_from(document.byte_size).ok() != Some(stored.byte_size)
    {
        let _ = tokio::fs::remove_file(&path).await;
        anyhow::bail!("stored document integrity check failed");
    }
    let parser = match parser {
        Some(parser) => parser,
        None => Arc::new(PdfiumParser::from_env()?),
    };
    if parser.version() != PARSER_VERSION {
        let _ = tokio::fs::remove_file(&path).await;
        anyhow::bail!("document parser version does not match the job");
    }
    let parse_path = path.clone();
    let parsed = tokio::task::spawn_blocking(move || parser.parse_file(&parse_path)).await;
    let _ = tokio::fs::remove_file(&path).await;
    Ok(parsed??)
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
