use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

use deepref_ai::{
    AiExecutionContext, AiGateway, AiTaskRunner, ProposalPersistence, ProviderEndpoint,
    SystemClock, UuidProvider,
};
use deepref_core::{IngestionItemStatus, normalize_doi};
use deepref_crossref::CrossrefError;
use deepref_documents::{
    DocumentParser, DocumentStore, HttpsPdfFetcher, PARSER_VERSION, PdfiumParser,
    RemoteDocumentFetcher,
};
use deepref_events::{DeadLetterRecord, EventEnvelope, WorkFetchRequested, deserialize_compatible};
use deepref_providers::CrossrefProvider;
use deepref_review::worker::{
    CompiledReview, ReviewExecutionPlan, ReviewModelIdentity, ReviewNode, ScreeningReviewPlan,
    StandardReviewPlan,
};
use serde::Deserialize;
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

async fn parse_work_fetch_event(
    pool: &PgPool,
    bytes: &[u8],
    delivery_count: u64,
) -> anyhow::Result<Result<EventEnvelope<WorkFetchRequested>, DeliveryAction>> {
    match deserialize_compatible(bytes) {
        Ok(event) => Ok(Ok(event)),
        Err(error) => {
            let record = dead_letter(bytes, None, delivery_count, "MALFORMED_PAYLOAD");
            store::persist_malformed_dead_letter(
                pool,
                &record,
                serde_json::json!({
                    "payload_utf8_lossy": String::from_utf8_lossy(bytes),
                    "error": error.to_string(),
                }),
            )
            .await?;
            Ok(Err(action_for(FailureClass::Malformed, delivery_count)))
        }
    }
}

enum PreconditionOutcome {
    Proceed,
    Action(DeliveryAction),
}

async fn check_work_fetch_preconditions(
    pool: &PgPool,
    event: &EventEnvelope<WorkFetchRequested>,
    doi: &str,
    owner: Uuid,
) -> anyhow::Result<PreconditionOutcome> {
    if event.payload.depth > event.payload.max_depth {
        store::complete_without_doi(
            pool,
            event,
            doi,
            owner,
            IngestionItemStatus::Skipped,
            Some("maximum depth exceeded"),
            None,
        )
        .await?;
        return Ok(PreconditionOutcome::Action(DeliveryAction::Ack));
    }
    if store::ingestion_cancelled(pool, event.payload.ingestion_id).await? {
        store::complete_without_doi(
            pool,
            event,
            doi,
            owner,
            IngestionItemStatus::Skipped,
            Some("ingestion cancelled"),
            None,
        )
        .await?;
        return Ok(PreconditionOutcome::Action(DeliveryAction::Ack));
    }
    if store::is_cached(pool, doi).await? {
        store::attach_cached(pool, event, doi, owner).await?;
        return Ok(PreconditionOutcome::Action(DeliveryAction::Ack));
    }
    Ok(PreconditionOutcome::Proceed)
}

async fn claim_doi_lease(
    pool: &PgPool,
    bytes: &[u8],
    event: &EventEnvelope<WorkFetchRequested>,
    doi: &str,
    owner: Uuid,
    claim_lease: Duration,
    delivery_count: u64,
) -> anyhow::Result<Option<DeliveryAction>> {
    if store::claim_doi(pool, doi, owner, claim_lease).await? != store::ClaimState::Acquired {
        let action = action_for(FailureClass::Retryable, delivery_count);
        if action == DeliveryAction::Terminate {
            let record = dead_letter(
                bytes,
                Some(event.event_id),
                delivery_count,
                "DOI_LEASE_EXHAUSTED",
            );
            store::complete_without_doi(
                pool,
                event,
                doi,
                owner,
                IngestionItemStatus::Failed,
                Some("DOI lease remained busy through the final delivery"),
                Some(&record),
            )
            .await?;
        } else {
            store::release_event_claim(pool, event.event_id, owner, "DOI lease is busy").await?;
        }
        return Ok(Some(action));
    }
    Ok(None)
}

async fn execute_work_fetch(
    pool: PgPool,
    bytes: &[u8],
    event: &EventEnvelope<WorkFetchRequested>,
    doi: &str,
    owner: Uuid,
    claim_lease: Duration,
    delivery_count: u64,
) -> anyhow::Result<DeliveryAction> {
    let settings = store::load_runtime_settings(&pool).await?;
    let client = match CrossrefProvider::new(settings.crossref_mailto.clone()) {
        Ok(client) => client.with_max_attempts(settings.retry_attempts),
        Err(error) => {
            store::finalize_terminal_failure(
                &pool,
                event,
                doi,
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
        doi.to_owned(),
        owner,
        claim_lease,
    );
    let fetched = client.fetch_work_with_references(doi).await;
    cancel_heartbeat.send_replace(true);
    let _ = heartbeat_done.await;

    match fetched {
        Ok(work) => {
            store::finalize_success(&pool, event, doi, owner, &work).await?;
            Ok(DeliveryAction::Ack)
        }
        Err(error) if is_retryable_crossref_error(&error) => {
            let action = action_for(FailureClass::Retryable, delivery_count);
            if action == DeliveryAction::Terminate {
                let record = dead_letter(
                    bytes,
                    Some(event.event_id),
                    delivery_count,
                    "DELIVERY_EXHAUSTED",
                );
                store::finalize_terminal_failure(
                    &pool,
                    event,
                    doi,
                    owner,
                    IngestionItemStatus::Failed,
                    &error.to_string(),
                    Some(&record),
                )
                .await?;
            } else {
                store::release_retryable(&pool, event.event_id, doi, owner, &error.to_string())
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
                event,
                doi,
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

pub async fn handle_message(
    pool: PgPool,
    bytes: Vec<u8>,
    delivery_count: u64,
    claim_lease: Duration,
) -> anyhow::Result<DeliveryAction> {
    let event = match parse_work_fetch_event(&pool, &bytes, delivery_count).await? {
        Ok(event) => event,
        Err(action) => return Ok(action),
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

    if let PreconditionOutcome::Action(action) =
        check_work_fetch_preconditions(&pool, &event, &doi, owner).await?
    {
        return Ok(action);
    }

    if let Some(action) = claim_doi_lease(
        &pool,
        &bytes,
        &event,
        &doi,
        owner,
        claim_lease,
        delivery_count,
    )
    .await?
    {
        return Ok(action);
    }

    execute_work_fetch(
        pool,
        &bytes,
        &event,
        &doi,
        owner,
        claim_lease,
        delivery_count,
    )
    .await
}

pub async fn handle_job(
    pool: sqlx::PgPool,
    job: &deepref_application::jobs::ClaimedJob,
    claim_lease: Duration,
) -> anyhow::Result<DeliveryAction> {
    handle_job_with_document_services_inner(pool, job, claim_lease, JobServices::default()).await
}

pub async fn handle_job_with_documents(
    pool: sqlx::PgPool,
    job: &deepref_application::jobs::ClaimedJob,
    claim_lease: Duration,
    document_store: Option<Arc<DocumentStore>>,
    document_parser: Option<Arc<dyn DocumentParser>>,
) -> anyhow::Result<DeliveryAction> {
    handle_job_with_document_services_inner(
        pool,
        job,
        claim_lease,
        JobServices {
            document_store,
            document_parser,
            ..JobServices::default()
        },
    )
    .await
}

pub async fn handle_job_with_documents_owned(
    pool: sqlx::PgPool,
    job: &deepref_application::jobs::ClaimedJob,
    owner: &str,
    claim_lease: Duration,
    document_store: Option<Arc<DocumentStore>>,
    document_parser: Option<Arc<dyn DocumentParser>>,
) -> anyhow::Result<DeliveryAction> {
    handle_job_with_document_services_inner(
        pool,
        job,
        claim_lease,
        JobServices {
            document_store,
            document_parser,
            owner: Some(owner),
            ..JobServices::default()
        },
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
    handle_job_with_document_services_inner(
        pool,
        job,
        claim_lease,
        JobServices {
            document_store,
            document_parser,
            remote_fetcher,
            ..JobServices::default()
        },
    )
    .await
}

pub async fn handle_job_with_documents_owned_and_ai(
    pool: sqlx::PgPool,
    job: &deepref_application::jobs::ClaimedJob,
    owner: &str,
    claim_lease: Duration,
    document_store: Option<Arc<DocumentStore>>,
    document_parser: Option<Arc<dyn DocumentParser>>,
    ai_gateway: Arc<dyn AiGateway>,
) -> anyhow::Result<DeliveryAction> {
    handle_job_with_document_services_inner(
        pool,
        job,
        claim_lease,
        JobServices {
            document_store,
            document_parser,
            owner: Some(owner),
            ai_gateway: Some(ai_gateway),
            ..JobServices::default()
        },
    )
    .await
}

#[derive(Default)]
struct JobServices<'a> {
    document_store: Option<Arc<DocumentStore>>,
    document_parser: Option<Arc<dyn DocumentParser>>,
    remote_fetcher: Option<Arc<dyn RemoteDocumentFetcher>>,
    owner: Option<&'a str>,
    ai_gateway: Option<Arc<dyn AiGateway>>,
}

async fn handle_retrieve_document_job(
    pool: &sqlx::PgPool,
    job: &deepref_application::jobs::ClaimedJob,
    document_store: Option<Arc<DocumentStore>>,
    remote_fetcher: Option<Arc<dyn RemoteDocumentFetcher>>,
) -> anyhow::Result<DeliveryAction> {
    let document_id = job
        .payload
        .get("document_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("retrieve_document payload is missing document_id"))?
        .parse::<Uuid>()?;
    let document = deepref_postgres::get_document_by_id(pool, document_id).await?;
    if document.object_key.is_some() && matches!(document.status.as_str(), "uploaded" | "available")
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
    if !deepref_postgres::mark_document_retrieving(pool, document_id).await? {
        return Ok(DeliveryAction::Ack);
    }
    let fetcher: Arc<dyn RemoteDocumentFetcher> =
        remote_fetcher.unwrap_or_else(|| Arc::new(HttpsPdfFetcher::default()));
    let (stored, _) = match fetcher.fetch(external_url, &store).await {
        Ok(result) => result,
        Err(error) => {
            // The researcher sees the reason in plain words; the raw error stays in the log.
            tracing::warn!(
                %document_id,
                permanent = error.is_permanent(),
                error = %error,
                "external document retrieval failed"
            );
            deepref_postgres::mark_document_retrieval_failed(
                pool,
                document_id,
                &error.failure_record(),
            )
            .await?;
            if error.is_permanent() {
                // Retrying the same link cannot succeed. The reason is recorded on the
                // document, so the job completes instead of retrying and dead-lettering.
                return Ok(DeliveryAction::Ack);
            }
            return Err(error.into());
        }
    };
    let byte_size = match i64::try_from(stored.byte_size) {
        Ok(value) => value,
        Err(error) => {
            let _ = store.delete(&stored.opaque_id).await;
            deepref_postgres::mark_document_retrieval_failed(
                pool,
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
        deepref_domain::ProjectId::new(document.project_id),
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
                pool,
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
                pool,
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

async fn handle_parse_document_job(
    pool: &sqlx::PgPool,
    job: &deepref_application::jobs::ClaimedJob,
    document_store: Option<Arc<DocumentStore>>,
    document_parser: Option<Arc<dyn DocumentParser>>,
) -> anyhow::Result<DeliveryAction> {
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
    // A reparse request is forced: it runs even when the active parser version
    // is already current, so the user gets fresh sections and references.
    let forced = job
        .payload
        .get("force")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let document = deepref_postgres::get_document_by_id(pool, document_id).await?;
    if !forced
        && document
            .active_parser_version
            .as_deref()
            .is_some_and(|active| active.starts_with(PARSER_VERSION))
    {
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
    deepref_postgres::mark_document_parsing(pool, document_id).await?;
    let parse_permit = pdf_parse_semaphore()
        .acquire_owned()
        .await
        .map_err(|error| anyhow::anyhow!("PDF parser concurrency limiter closed: {error}"))?;
    let parsed =
        match parse_stored_document(&document, object_key, Arc::clone(&store), document_parser)
            .await
        {
            Ok(parsed) => parsed,
            Err(error) => {
                drop(parse_permit);
                deepref_postgres::mark_document_failed(pool, document_id, &error.to_string())
                    .await?;
                return Err(error);
            }
        };
    drop(parse_permit);
    let (structure, parser_version, header) =
        grobid_enrichment(document_id, object_key, &store, &parsed).await;
    let pdf_identity = deepref_documents::PdfIdentity::from_parse(
        &parsed,
        header.title.as_deref(),
        header.doi.as_deref(),
    );
    let parsed = parsed.document;
    deepref_postgres::persist_parsed_document_with_structure(
        pool,
        document_id,
        &parsed,
        &parser_version,
        &structure,
    )
    .await?;
    record_identity_check(pool, document_id, document.report_id, &pdf_identity).await;
    Ok(DeliveryAction::Ack)
}

/// Compares the parsed PDF with its report and stores the verdict. Advisory only: a failure here
/// is logged and never fails the parse.
async fn record_identity_check(
    pool: &sqlx::PgPool,
    document_id: Uuid,
    report_id: Uuid,
    pdf: &deepref_documents::PdfIdentity,
) {
    let outcome = async {
        let (title, doi) = deepref_postgres::report_identity(pool, report_id).await?;
        let check = deepref_documents::check_identity(
            pdf,
            &deepref_documents::ReportIdentity {
                title: title.as_deref(),
                doi: doi.as_deref(),
            },
        );
        deepref_postgres::record_identity_check(pool, document_id, &check).await?;
        anyhow::Ok(check.verdict)
    }
    .await;
    match outcome {
        Ok(verdict) => {
            tracing::info!(%document_id, ?verdict, "document identity checked against its report");
        }
        Err(error) => {
            tracing::warn!(%document_id, %error, "document identity check failed");
        }
    }
}

/// What GROBID's header says about the article (empty when GROBID did not run).
#[derive(Debug, Default)]
struct GrobidHeader {
    title: Option<String>,
    doi: Option<String>,
}

/// Runs the optional GROBID enrichment. Never fails the job: on any problem it
/// logs one warning and returns an empty structure under the native version.
async fn grobid_enrichment(
    document_id: Uuid,
    object_key: &str,
    store: &DocumentStore,
    structured: &deepref_documents::StructuredDocument,
) -> (deepref_postgres::DocumentStructure, String, GrobidHeader) {
    let parsed = &structured.document;
    let native = (
        crate::enrichment::native_structure(structured),
        PARSER_VERSION.to_owned(),
        GrobidHeader::default(),
    );
    let client = match crate::enrichment::grobid_client_from_env() {
        Ok(Some(client)) => client,
        Ok(None) => return native,
        Err(error) => {
            tracing::warn!(%document_id, %error, "GROBID enrichment skipped: invalid configuration");
            return native;
        }
    };
    let mut pdf = Vec::new();
    if let Err(error) = store.read_to_writer(object_key, &mut pdf).await {
        tracing::warn!(%document_id, %error, "GROBID enrichment skipped: could not re-read the PDF");
        return native;
    }
    let tei = match client.process_fulltext(&pdf).await {
        Ok(tei) => tei,
        Err(error) => {
            tracing::warn!(%document_id, %error, "GROBID enrichment failed; keeping the native parse");
            return native;
        }
    };
    let header = GrobidHeader {
        title: tei.title.clone(),
        doi: tei.doi.clone(),
    };
    let views: Vec<crate::enrichment::BlockView<'_>> = parsed
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| crate::enrichment::BlockView {
            ordinal: block.ordinal,
            text: &block.text,
            native_role: structured
                .block_structure
                .get(index)
                .map_or("", |native| native.role.as_str()),
            native_path: structured
                .block_structure
                .get(index)
                .map_or(&[][..], |native| native.section_path.as_slice()),
        })
        .collect();
    let mut enrichment = crate::enrichment::build_structure(&views, &tei);
    crate::enrichment::overlay_on_native(&mut enrichment.structure, structured);
    tracing::info!(
        %document_id,
        sections = enrichment.structure.sections.len(),
        references = enrichment.structure.references.len(),
        labelled_blocks = enrichment.structure.section_paths.len(),
        "GROBID enrichment applied"
    );
    let version = format!("{PARSER_VERSION}{}", enrichment.parser_suffix);
    (enrichment.structure, version, header)
}

async fn handle_recompute_metrics_job(
    pool: &sqlx::PgPool,
    payload: serde_json::Value,
) -> anyhow::Result<DeliveryAction> {
    let event: EventEnvelope<deepref_events::DomainPayload> = serde_json::from_value(payload)?;
    let project_id = match event.payload {
        deepref_events::DomainPayload::MetricsRecomputeRequested(p) => p.project_id,
        _ => anyhow::bail!("recompute_metrics job has an unsupported payload"),
    };
    deepref_postgres::recompute_project_metrics(pool, project_id).await?;
    Ok(DeliveryAction::Ack)
}

/// Upper bound on passes for one import, so a huge import cannot pin a worker.
const IMPORT_DEDUPLICATION_MAX_PASSES: usize = 50;

/// Runs the duplicate check queued after a file import. Each pass resolves at
/// most 100 records and the loop stops at the first pass that is not full.
async fn handle_import_deduplication_job(
    pool: &sqlx::PgPool,
    payload: serde_json::Value,
) -> anyhow::Result<DeliveryAction> {
    #[derive(Deserialize)]
    struct ImportDeduplicationPayload {
        project_id: Uuid,
    }
    let payload: ImportDeduplicationPayload = serde_json::from_value(payload)?;
    for _ in 0..IMPORT_DEDUPLICATION_MAX_PASSES {
        let summary = deepref_postgres::run_deduplication(
            pool,
            deepref_postgres::DedupeRunRequest {
                project_id: payload.project_id,
                limit: 100,
                actor_kind: "automation".to_owned(),
                actor_id: "import-deduplication".to_owned(),
            },
        )
        .await?;
        if summary.processed < 100 {
            break;
        }
    }
    Ok(DeliveryAction::Ack)
}

async fn handle_job_with_document_services_inner(
    pool: sqlx::PgPool,
    job: &deepref_application::jobs::ClaimedJob,
    claim_lease: Duration,
    services: JobServices<'_>,
) -> anyhow::Result<DeliveryAction> {
    let JobServices {
        document_store,
        document_parser,
        remote_fetcher,
        owner,
        ai_gateway,
    } = services;
    match job.kind.as_str() {
        "work_fetch_requested" => {
            let bytes = serde_json::to_vec(&job.payload)?;
            handle_message(pool, bytes, job.attempts.max(1) as u64, claim_lease).await
        }
        "recompute_metrics" => handle_recompute_metrics_job(&pool, job.payload.clone()).await,
        deepref_postgres::IMPORT_DEDUPLICATION_JOB_KIND => {
            handle_import_deduplication_job(&pool, job.payload.clone()).await
        }
        deepref_postgres::PMID_IMPORT_JOB_KIND => {
            crate::pmid_import::handle_job(
                &pool,
                job.payload.clone(),
                job.attempts,
                job.max_attempts,
            )
            .await
        }
        "automation_run" => {
            let owner = owner.ok_or_else(|| {
                anyhow::anyhow!("automation job processing requires its lease owner")
            })?;
            handle_automation_run(&pool, job, owner, ai_gateway.as_deref()).await
        }
        "workflow_step" => crate::workflows::handle_step(&pool, job, ai_gateway.clone()).await,
        "recompute_prisma" => Ok(DeliveryAction::Ack),
        "retrieve_document" => {
            handle_retrieve_document_job(&pool, job, document_store, remote_fetcher).await
        }
        "parse_document" => {
            handle_parse_document_job(&pool, job, document_store, document_parser).await
        }
        other => anyhow::bail!("unsupported durable job kind: {other}"),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AutomationRunJobPayload {
    automation_run_id: Uuid,
}

const UNKNOWN_AUTOMATION_STEP_ERROR: &str =
    "automation step is not an accepted built-in deterministic action";

async fn handle_automation_run(
    pool: &sqlx::PgPool,
    job: &deepref_application::jobs::ClaimedJob,
    owner: &str,
    ai_gateway: Option<&dyn AiGateway>,
) -> anyhow::Result<DeliveryAction> {
    let payload: AutomationRunJobPayload = serde_json::from_value(job.payload.clone())?;
    let run_id = deepref_application::AutomationRunId::new(payload.automation_run_id)
        .map_err(|error| anyhow::anyhow!("automation run id is invalid: {error}"))?;
    let project_id = deepref_postgres::get_claimed_automation_job_project_id_for_run(
        pool, job.id, run_id, owner,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("automation job and run association is invalid"))?;
    let project_uuid = project_id.as_uuid();

    loop {
        let Some(step) =
            deepref_postgres::begin_next_automation_step(pool, project_id, run_id, owner).await?
        else {
            deepref_postgres::finalize_automation_run(pool, project_id, run_id).await?;
            return Ok(DeliveryAction::Ack);
        };

        if step.key == "recompute_project_metrics"
            && step.kind == deepref_application::AutomationStepKind::DeterministicAction
        {
            deepref_postgres::recompute_project_metrics(pool, project_uuid).await?;
            deepref_postgres::complete_automation_step(pool, project_id, step.id, owner).await?;
            continue;
        }

        if step.key == "execute_compiled_review"
            && step.kind == deepref_application::AutomationStepKind::AiTask
        {
            let gateway = ai_gateway.ok_or_else(|| {
                anyhow::anyhow!("compiled review execution requires an AI gateway")
            })?;
            match execute_compiled_review(pool, project_id, run_id, &step, owner, gateway).await {
                Ok(()) => continue,
                Err(error) if job.attempts < job.max_attempts => return Err(error),
                Err(error) => {
                    let message = bounded_worker_error(&error);
                    deepref_postgres::fail_review_run(
                        pool,
                        project_id,
                        deepref_review::ReviewRunId::new(run_id.as_uuid())?,
                        "review_execution_failed",
                        &message,
                    )
                    .await?;
                    deepref_postgres::fail_automation_step(
                        pool, project_id, step.id, owner, &message,
                    )
                    .await?;
                    deepref_postgres::finalize_automation_run(pool, project_id, run_id).await?;
                    return Ok(DeliveryAction::Terminate);
                }
            }
        }

        deepref_postgres::fail_automation_step(
            pool,
            project_id,
            step.id,
            owner,
            UNKNOWN_AUTOMATION_STEP_ERROR,
        )
        .await?;
        deepref_postgres::finalize_automation_run(pool, project_id, run_id).await?;
        return Ok(DeliveryAction::Terminate);
    }
}

async fn execute_compiled_review(
    pool: &sqlx::PgPool,
    project_id: deepref_domain::ProjectId,
    automation_run_id: deepref_application::AutomationRunId,
    automation_step: &deepref_application::AutomationStepRun,
    owner: &str,
    gateway: &dyn AiGateway,
) -> anyhow::Result<()> {
    let review_run_id = deepref_review::ReviewRunId::new(automation_run_id.as_uuid())?;
    let run =
        deepref_postgres::load_leased_review_run(pool, project_id, review_run_id, owner).await?;
    let review = CompiledReview::compile(run.snapshot.definition)?;
    if let deepref_review::ReviewRunState::Completed { .. }
    | deepref_review::ReviewRunState::Blocked { .. } = run.snapshot.state
    {
        let accepted = latest_accepted_review_attempt(
            pool,
            project_id,
            review_run_id,
            final_review_node(&review).id(),
        )
        .await?;
        deepref_postgres::complete_review_step(
            pool,
            project_id,
            automation_step.id,
            accepted,
            owner,
            Some(serde_json::to_value(&run.snapshot)?),
        )
        .await?;
        return Ok(());
    }
    deepref_postgres::mark_review_run_running(pool, project_id, review_run_id, owner).await?;

    let prepare = persist_review_node(
        pool,
        &run,
        &review,
        owner,
        ReviewNodeWrite {
            node: review.plan().prepare(),
            payload: serde_json::to_value(&run.task)?,
            predecessors: &[],
            model_run_id: None,
        },
    )
    .await?;
    // Checked before any model call: the scheduler and this worker may have different
    // configuration, and the review identity names the endpoint it was scheduled against.
    if let Some(route) =
        provider_endpoint_mismatch(&run.manifest.resolved_models, registered_provider_endpoint)
    {
        tracing::warn!(
            profile = route.profile.as_str(),
            "review provider endpoint differs from the endpoint it was scheduled against; the model was not called"
        );
        return finalize_blocked_review(
            pool,
            &run,
            &review,
            final_review_node(&review),
            automation_step,
            owner,
            BlockedReview {
                predecessor: &prepare,
                code: deepref_review::ReviewBlockCode::ProviderEndpointMismatch,
                message: "the AI provider endpoint differs from the endpoint this review was scheduled against, so the model was not called",
            },
        )
        .await;
    }
    let execution = CompiledReviewExecution {
        pool,
        run: &run,
        review: &review,
        automation_step,
        owner,
        gateway,
    };
    match review.plan() {
        ReviewExecutionPlan::Screening(plan) => {
            execute_compiled_screening(&execution, plan, prepare).await
        }
        ReviewExecutionPlan::Standard(plan) => {
            execute_compiled_standard(&execution, plan, prepare).await
        }
    }
}

/// The endpoint this worker calls for `provider`. A registry that cannot be read reports no
/// endpoint, so the comparison fails closed.
fn registered_provider_endpoint(provider: &str) -> Option<ProviderEndpoint> {
    deepref_ai::provider_endpoint(provider).ok().flatten()
}

/// The first resolved route whose recorded endpoint is not the endpoint `worker_endpoint` gives
/// for its provider. A configured worker also refuses a route whose scheduler
/// did not record an endpoint; it cannot establish which endpoint was calibrated.
/// Two absent endpoints are permitted for unconfigured/in-process fixture gateways.
fn provider_endpoint_mismatch(
    routes: &[ReviewModelIdentity],
    worker_endpoint: impl Fn(&str) -> Option<ProviderEndpoint>,
) -> Option<&ReviewModelIdentity> {
    routes
        .iter()
        .find(|route| worker_endpoint(&route.provider).as_ref() != route.endpoint.as_ref())
}

struct CompiledReviewExecution<'a> {
    pool: &'a sqlx::PgPool,
    run: &'a deepref_postgres::LeasedReviewRun,
    review: &'a CompiledReview,
    automation_step: &'a deepref_application::AutomationStepRun,
    owner: &'a str,
    gateway: &'a dyn AiGateway,
}

async fn execute_compiled_standard(
    execution: &CompiledReviewExecution<'_>,
    plan: &StandardReviewPlan,
    prepare: AcceptedNodeArtifact,
) -> anyhow::Result<()> {
    let pool = execution.pool;
    let run = execution.run;
    let review = execution.review;
    let automation_step = execution.automation_step;
    let owner = execution.owner;
    let gateway = execution.gateway;
    let generated = execute_review_ai_node(
        pool,
        run,
        review,
        &plan.generate,
        std::slice::from_ref(&prepare),
        owner,
        gateway,
    )
    .await?;
    let validated = persist_review_node(
        pool,
        run,
        review,
        owner,
        ReviewNodeWrite {
            node: &plan.validate,
            payload: serde_json::json!({
                "model_run_id": generated.model_run_id,
                "output_hash": deepref_ai::hash_json(&generated.executed.output)?,
                "semantic_validation": "passed"
            }),
            predecessors: std::slice::from_ref(&generated.artifact),
            model_run_id: None,
        },
    )
    .await?;
    let assembled = persist_review_node(
        pool,
        run,
        review,
        owner,
        ReviewNodeWrite {
            node: &plan.assemble,
            payload: serde_json::to_value(&generated.executed)?,
            predecessors: std::slice::from_ref(&validated),
            model_run_id: Some(generated.model_run_id),
        },
    )
    .await?;

    finalize_compiled_candidate(
        pool,
        run,
        review,
        ReviewFinalizationRequest {
            predecessor: &assembled,
            executed: generated.executed,
            finalize_node: &plan.finalize,
        },
        automation_step,
        owner,
    )
    .await
}

async fn execute_compiled_screening(
    execution: &CompiledReviewExecution<'_>,
    plan: &ScreeningReviewPlan,
    prepare: AcceptedNodeArtifact,
) -> anyhow::Result<()> {
    let pool = execution.pool;
    let run = execution.run;
    let review = execution.review;
    let automation_step = execution.automation_step;
    let owner = execution.owner;
    let gateway = execution.gateway;
    let primary = execute_review_ai_node(
        pool,
        run,
        review,
        &plan.primary_screen,
        std::slice::from_ref(&prepare),
        owner,
        gateway,
    )
    .await?;
    let primary_analysis = screening_analysis(&primary)?;
    let validated_primary = persist_review_node(
        pool,
        run,
        review,
        owner,
        ReviewNodeWrite {
            node: &plan.validate_primary,
            payload: serde_json::json!({
                "model_run_id": primary.model_run_id,
                "output_hash": deepref_ai::hash_json(&primary.executed.output)?,
                "semantic_validation":"passed"
            }),
            predecessors: std::slice::from_ref(&primary.artifact),
            model_run_id: None,
        },
    )
    .await?;
    let needs_independent = deepref_review::worker::needs_independent_screen(&primary_analysis);
    let derived = persist_review_node(
        pool,
        run,
        review,
        owner,
        ReviewNodeWrite {
            node: &plan.derive_primary,
            payload: serde_json::json!({
                "suggested_decision": primary_analysis.suggested_decision,
                "needs_independent_screen": needs_independent
            }),
            predecessors: std::slice::from_ref(&validated_primary),
            model_run_id: None,
        },
    )
    .await?;

    let (mut candidate, reconciliation) = if needs_independent {
        // The independent task receives only the immutable prepared source. The
        // primary artifact affects its fingerprint and lineage, never its model context.
        let independent = execute_review_ai_node(
            pool,
            run,
            review,
            &plan.independent_screen,
            std::slice::from_ref(&derived),
            owner,
            gateway,
        )
        .await?;
        let independent_analysis = screening_analysis(&independent)?;
        let validated_independent = persist_review_node(
            pool,
            run,
            review,
            owner,
            ReviewNodeWrite {
                node: &plan.validate_independent,
                payload: serde_json::json!({
                    "model_run_id": independent.model_run_id,
                    "output_hash": deepref_ai::hash_json(&independent.executed.output)?,
                    "semantic_validation":"passed"
                }),
                predecessors: std::slice::from_ref(&independent.artifact),
                model_run_id: None,
            },
        )
        .await?;
        let agreement =
            primary_analysis.suggested_decision == independent_analysis.suggested_decision;
        let reconciliation = persist_review_node(
            pool,
            run,
            review,
            owner,
            ReviewNodeWrite {
                node: &plan.reconcile,
                payload: serde_json::json!({
                    "agreement": agreement,
                    "primary_decision": primary_analysis.suggested_decision,
                    "independent_decision": independent_analysis.suggested_decision,
                    "authority": if agreement { "deterministic_agreement" } else { "human_adjudication_required" }
                }),
                predecessors: std::slice::from_ref(&validated_independent),
                model_run_id: None,
            },
        )
        .await?;
        if !agreement {
            return finalize_blocked_review(
                pool,
                run,
                review,
                &plan.finalize,
                automation_step,
                owner,
                BlockedReview {
                    predecessor: &reconciliation,
                    code: deepref_review::ReviewBlockCode::HumanAdjudicationRequired,
                    message: "independent screening disagreed with the primary screening",
                },
            )
            .await;
        }
        (primary, reconciliation)
    } else {
        (primary, derived)
    };

    let protected_decision = screening_analysis(&candidate)?.suggested_decision;
    let mut predecessor = reconciliation;
    for repair_cycle in 0..=plan.repair_budget {
        let assembled = persist_review_node(
            pool,
            run,
            review,
            owner,
            ReviewNodeWrite {
                node: &plan.assemble,
                payload: serde_json::to_value(&candidate.executed)?,
                predecessors: std::slice::from_ref(&predecessor),
                model_run_id: Some(candidate.model_run_id),
            },
        )
        .await?;
        let candidate_hash = deepref_ai::hash_json(&candidate.executed.output)?;
        let audit = execute_review_ai_node_with_context(
            pool,
            run,
            review,
            owner,
            gateway,
            ReviewAiNodeRequest {
                node: &plan.candidate_audit,
                predecessors: std::slice::from_ref(&assembled),
                semantic_context: Some(serde_json::json!({
                    "candidate_hash": candidate_hash,
                    "candidate": candidate.executed.output.clone()
                })),
            },
        )
        .await?;
        let audit_decision = screening_analysis(&audit)?.suggested_decision;
        if audit_decision == protected_decision {
            return finalize_compiled_candidate(
                pool,
                run,
                review,
                ReviewFinalizationRequest {
                    predecessor: &audit.artifact,
                    executed: candidate.executed,
                    finalize_node: &plan.finalize,
                },
                automation_step,
                owner,
            )
            .await;
        }
        if repair_cycle == plan.repair_budget {
            return finalize_blocked_review(
                pool,
                run,
                review,
                &plan.finalize,
                automation_step,
                owner,
                BlockedReview {
                    predecessor: &audit.artifact,
                    code: deepref_review::ReviewBlockCode::RepairBudgetExhausted,
                    message: "candidate audit did not pass within the bounded semantic repair budget",
                },
            )
            .await;
        }
        let repair = execute_review_ai_node_with_context(
            pool,
            run,
            review,
            owner,
            gateway,
            ReviewAiNodeRequest {
                node: &plan.semantic_repair,
                predecessors: std::slice::from_ref(&audit.artifact),
                semantic_context: Some(serde_json::json!({
                    "candidate_hash": candidate_hash,
                    "candidate": candidate.executed.output.clone(),
                    "audit": audit.executed.output.clone(),
                    "protected_decision": protected_decision.clone()
                })),
            },
        )
        .await?;
        let repaired = screening_analysis(&repair)?;
        let protected_decision_unchanged = repaired.suggested_decision == protected_decision;
        predecessor = persist_review_node(
            pool,
            run,
            review,
            owner,
            ReviewNodeWrite {
                node: &plan.validate_repair,
                payload: serde_json::json!({
                    "repair_cycle": repair_cycle + 1,
                    "protected_decision_unchanged": protected_decision_unchanged,
                    "output_hash": deepref_ai::hash_json(&repair.executed.output)?
                }),
                predecessors: std::slice::from_ref(&repair.artifact),
                model_run_id: None,
            },
        )
        .await?;
        if protected_decision_unchanged {
            // Only the semantic judgments, rationales, evidence, and
            // uncertainties come from the repair. Identity and decision stay protected.
            candidate = repair;
        }
    }
    unreachable!("bounded screening repair loop always returns")
}

fn screening_analysis(node: &GeneratedReviewNode) -> anyhow::Result<deepref_ai::ScreeningAnalysis> {
    serde_json::from_value(node.executed.output.clone())
        .map_err(|error| anyhow::anyhow!("stored screening output is invalid: {error}"))
}

async fn finalize_compiled_candidate(
    pool: &sqlx::PgPool,
    run: &deepref_postgres::LeasedReviewRun,
    review: &CompiledReview,
    request: ReviewFinalizationRequest<'_>,
    automation_step: &deepref_application::AutomationStepRun,
    owner: &str,
) -> anyhow::Result<()> {
    let ReviewFinalizationRequest {
        predecessor,
        executed,
        finalize_node,
    } = request;
    if executed.proposal.operation != review.final_proposal_type() {
        anyhow::bail!(
            "compiled review produced proposal type {} instead of {}",
            executed.proposal.operation,
            review.final_proposal_type()
        );
    }
    let predecessor_input = artifact_input(predecessor);
    let final_start = deepref_postgres::begin_review_attempt(
        pool,
        run,
        review,
        finalize_node,
        std::slice::from_ref(&predecessor_input),
        owner,
    )
    .await?;
    let final_attempt = match final_start {
        deepref_postgres::ReviewAttemptStart::Reused { attempt_id, .. } => attempt_id,
        deepref_postgres::ReviewAttemptStart::Started { attempt_id, .. } => {
            let outcome = deepref_postgres::complete_review_outcome(
                pool,
                run,
                deepref_postgres::ReviewOutcomeCompletion {
                    final_attempt_id: attempt_id,
                    predecessors: std::slice::from_ref(&predecessor_input),
                    outcome: deepref_postgres::ReviewOutcome::Candidate {
                        proposal: deepref_ai::AiProposal {
                            id: Uuid::new_v4(),
                            draft: executed.proposal,
                            model_run_id: executed.model_run_id,
                            status: deepref_ai::ProposalStatus::Pending,
                            resolved_at: None,
                            resolved_by_actor_id: None,
                        },
                    },
                    automation_step_id: automation_step.id,
                    worker_id: owner,
                },
            )
            .await?;
            return match outcome {
                deepref_postgres::ReviewFinalization::Completed { .. }
                | deepref_postgres::ReviewFinalization::Blocked => Ok(()),
            };
        }
    };
    complete_review_automation_step(pool, run, automation_step, final_attempt, owner).await
}

struct ReviewFinalizationRequest<'a> {
    predecessor: &'a AcceptedNodeArtifact,
    executed: deepref_review::worker::ExecutedReviewTask,
    finalize_node: &'a ReviewNode,
}

async fn finalize_blocked_review(
    pool: &sqlx::PgPool,
    run: &deepref_postgres::LeasedReviewRun,
    review: &CompiledReview,
    finalize_node: &ReviewNode,
    automation_step: &deepref_application::AutomationStepRun,
    owner: &str,
    blocked: BlockedReview<'_>,
) -> anyhow::Result<()> {
    let BlockedReview {
        predecessor,
        code,
        message,
    } = blocked;
    let predecessor_input = artifact_input(predecessor);
    let final_start = deepref_postgres::begin_review_attempt(
        pool,
        run,
        review,
        finalize_node,
        std::slice::from_ref(&predecessor_input),
        owner,
    )
    .await?;
    match final_start {
        deepref_postgres::ReviewAttemptStart::Reused { attempt_id, .. } => {
            complete_review_automation_step(pool, run, automation_step, attempt_id, owner).await
        }
        deepref_postgres::ReviewAttemptStart::Started { attempt_id, .. } => {
            deepref_postgres::complete_review_outcome(
                pool,
                run,
                deepref_postgres::ReviewOutcomeCompletion {
                    final_attempt_id: attempt_id,
                    predecessors: std::slice::from_ref(&predecessor_input),
                    outcome: deepref_postgres::ReviewOutcome::Blocked { code, message },
                    automation_step_id: automation_step.id,
                    worker_id: owner,
                },
            )
            .await?;
            Ok(())
        }
    }
}

async fn complete_review_automation_step(
    pool: &sqlx::PgPool,
    run: &deepref_postgres::LeasedReviewRun,
    automation_step: &deepref_application::AutomationStepRun,
    final_attempt: Uuid,
    owner: &str,
) -> anyhow::Result<()> {
    deepref_postgres::complete_review_step(
        pool,
        run.snapshot.project_id,
        automation_step.id,
        final_attempt,
        owner,
        Some(serde_json::to_value(
            &deepref_postgres::get_review_run(pool, run.snapshot.project_id, run.snapshot.id)
                .await?,
        )?),
    )
    .await?;
    Ok(())
}

#[derive(Clone)]
struct AcceptedNodeArtifact {
    artifact_id: Uuid,
    artifact_hash: deepref_review::worker::ReviewHash,
}

#[derive(Clone)]
struct GeneratedReviewNode {
    executed: deepref_review::worker::ExecutedReviewTask,
    model_run_id: Uuid,
    artifact: AcceptedNodeArtifact,
}

struct BlockedReview<'a> {
    predecessor: &'a AcceptedNodeArtifact,
    code: deepref_review::ReviewBlockCode,
    message: &'a str,
}

struct ReviewNodeWrite<'a> {
    node: &'a ReviewNode,
    payload: serde_json::Value,
    predecessors: &'a [AcceptedNodeArtifact],
    model_run_id: Option<Uuid>,
}

async fn persist_review_node(
    pool: &sqlx::PgPool,
    run: &deepref_postgres::LeasedReviewRun,
    review: &CompiledReview,
    owner: &str,
    write: ReviewNodeWrite<'_>,
) -> anyhow::Result<AcceptedNodeArtifact> {
    let ReviewNodeWrite {
        node,
        payload,
        predecessors,
        model_run_id,
    } = write;
    let predecessor_inputs = predecessors.iter().map(artifact_input).collect::<Vec<_>>();
    match deepref_postgres::begin_review_attempt(
        pool,
        run,
        review,
        node,
        &predecessor_inputs,
        owner,
    )
    .await?
    {
        deepref_postgres::ReviewAttemptStart::Reused {
            artifact_id,
            artifact_hash,
            ..
        } => Ok(AcceptedNodeArtifact {
            artifact_id,
            artifact_hash,
        }),
        deepref_postgres::ReviewAttemptStart::Started { attempt_id, .. } => {
            let accepted = deepref_postgres::complete_review_attempt(
                pool,
                run,
                deepref_postgres::ReviewAttemptCompletion {
                    attempt_id,
                    payload,
                    media_type: "application/vnd.deepref.review-artifact+json",
                    predecessors: &predecessor_inputs,
                    model_run_id,
                    worker_id: owner,
                },
            )
            .await?;
            Ok(AcceptedNodeArtifact {
                artifact_id: accepted.artifact_id,
                artifact_hash: accepted.artifact_hash,
            })
        }
    }
}

async fn execute_review_ai_node(
    pool: &sqlx::PgPool,
    run: &deepref_postgres::LeasedReviewRun,
    review: &CompiledReview,
    node: &ReviewNode,
    predecessors: &[AcceptedNodeArtifact],
    owner: &str,
    gateway: &dyn AiGateway,
) -> anyhow::Result<GeneratedReviewNode> {
    execute_review_ai_node_with_context(
        pool,
        run,
        review,
        owner,
        gateway,
        ReviewAiNodeRequest {
            node,
            predecessors,
            semantic_context: None,
        },
    )
    .await
}

struct ReviewAiNodeRequest<'a> {
    node: &'a ReviewNode,
    predecessors: &'a [AcceptedNodeArtifact],
    semantic_context: Option<serde_json::Value>,
}

async fn execute_review_ai_node_with_context(
    pool: &sqlx::PgPool,
    run: &deepref_postgres::LeasedReviewRun,
    review: &CompiledReview,
    owner: &str,
    gateway: &dyn AiGateway,
    request: ReviewAiNodeRequest<'_>,
) -> anyhow::Result<GeneratedReviewNode> {
    let ReviewAiNodeRequest {
        node,
        predecessors,
        semantic_context,
    } = request;
    let predecessor_inputs = predecessors.iter().map(artifact_input).collect::<Vec<_>>();
    let start =
        deepref_postgres::begin_review_attempt(pool, run, review, node, &predecessor_inputs, owner)
            .await?;
    match start {
        deepref_postgres::ReviewAttemptStart::Reused {
            artifact_id,
            artifact_hash,
            payload,
            ..
        } => {
            let executed =
                serde_json::from_value::<deepref_review::worker::ExecutedReviewTask>(payload)?;
            Ok(GeneratedReviewNode {
                model_run_id: executed.model_run_id,
                executed,
                artifact: AcceptedNodeArtifact {
                    artifact_id,
                    artifact_hash,
                },
            })
        }
        deepref_postgres::ReviewAttemptStart::Started {
            attempt_id,
            input_fingerprint,
            ..
        } => {
            let store = deepref_postgres::PostgresAiStore::new(pool);
            let runner = AiTaskRunner::new(
                gateway,
                &store,
                &store,
                &store,
                &store,
                &SystemClock,
                &UuidProvider,
            );
            let execution = AiExecutionContext {
                parent_automation_run_id: Some(run.snapshot.id.as_uuid()),
                node_fingerprint: Some(input_fingerprint.to_string()),
                proposal_persistence: ProposalPersistence::Skip,
            };
            let executed = match run
                .task
                .execute_for_node(&runner, execution, node.id(), semantic_context)
                .await
            {
                Ok(executed) => executed,
                Err(error) => {
                    deepref_postgres::fail_review_attempt(
                        pool,
                        run,
                        attempt_id,
                        "ai_task_failed",
                        &bounded_message(&error.to_string(), 4_096),
                        owner,
                    )
                    .await?;
                    return Err(error.into());
                }
            };
            let model_run_id = executed.model_run_id;
            let accepted = deepref_postgres::complete_review_attempt(
                pool,
                run,
                deepref_postgres::ReviewAttemptCompletion {
                    attempt_id,
                    payload: serde_json::to_value(&executed)?,
                    media_type: "application/vnd.deepref.executed-review-task+json",
                    predecessors: &predecessor_inputs,
                    model_run_id: Some(model_run_id),
                    worker_id: owner,
                },
            )
            .await?;
            Ok(GeneratedReviewNode {
                executed,
                model_run_id,
                artifact: AcceptedNodeArtifact {
                    artifact_id: accepted.artifact_id,
                    artifact_hash: accepted.artifact_hash,
                },
            })
        }
    }
}

fn artifact_input(
    artifact: &AcceptedNodeArtifact,
) -> deepref_review::worker::AcceptedArtifactInput {
    deepref_review::worker::AcceptedArtifactInput {
        artifact_id: artifact.artifact_id,
        content_hash: artifact.artifact_hash.clone(),
    }
}

async fn latest_accepted_review_attempt(
    pool: &sqlx::PgPool,
    project_id: deepref_domain::ProjectId,
    review_run_id: deepref_review::ReviewRunId,
    final_node_id: &str,
) -> anyhow::Result<Uuid> {
    sqlx::query_scalar(
        "SELECT id FROM review_step_attempts
         WHERE project_id=$1 AND automation_run_id=$2 AND node_id=$3
           AND accepted_at IS NOT NULL
         ORDER BY accepted_at DESC,id DESC LIMIT 1",
    )
    .bind(project_id.as_uuid())
    .bind(review_run_id.as_uuid())
    .bind(final_node_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| anyhow::anyhow!("terminal review run has no accepted attempt"))
}

fn final_review_node(review: &CompiledReview) -> &ReviewNode {
    match review.plan() {
        ReviewExecutionPlan::Standard(plan) => &plan.finalize,
        ReviewExecutionPlan::Screening(plan) => &plan.finalize,
    }
}

fn bounded_worker_error(error: &anyhow::Error) -> String {
    bounded_message(&error.to_string(), 4_096)
}

fn bounded_message(message: &str, max_bytes: usize) -> String {
    if message.len() <= max_bytes {
        return message.to_owned();
    }
    let mut end = max_bytes;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message.get(..end).unwrap_or("").to_owned()
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
) -> anyhow::Result<deepref_documents::StructuredDocument> {
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
            if error.is_not_found() {
                anyhow::bail!(
                    "{}: the stored PDF file is no longer available; upload the PDF again",
                    deepref_documents::DOCUMENT_BLOB_MISSING_CODE
                );
            }
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
        None => PdfiumParser::shared_from_env()?,
    };
    if parser.version() != PARSER_VERSION {
        let _ = tokio::fs::remove_file(&path).await;
        anyhow::bail!("document parser version does not match the job");
    }
    let parse_path = path.clone();
    let parsed = tokio::task::spawn_blocking(move || parser.parse_structured(&parse_path)).await;
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
    use deepref_ai::ModelProfile;
    use deepref_review::worker::ReviewHash;

    use super::*;

    #[test]
    fn malformed_identity_is_stable() {
        assert_eq!(
            dead_letter(b"bad", None, 1, "bad").identity,
            dead_letter(b"bad", None, 5, "other").identity
        );
    }

    fn endpoint(raw: &str) -> ProviderEndpoint {
        ProviderEndpoint::from_configured_url(raw).unwrap()
    }

    fn route(provider: &str, endpoint: Option<ProviderEndpoint>) -> ReviewModelIdentity {
        ReviewModelIdentity {
            profile: ModelProfile::Reasoning,
            provider: provider.to_owned(),
            model: "glm-test".to_owned(),
            model_version: "glm-test".to_owned(),
            parameters_hash: ReviewHash::digest_bytes(b"parameters"),
            endpoint,
        }
    }

    #[test]
    fn a_worker_on_the_scheduled_endpoint_may_execute_the_review() {
        let scheduled = endpoint("https://opencode.ai/zen/go/v1");
        let routes = [route("opencode-go", Some(scheduled.clone()))];
        let worker = |provider: &str| {
            (provider == "opencode-go").then(|| endpoint("https://opencode.ai/zen/go/v1/"))
        };
        assert!(provider_endpoint_mismatch(&routes, worker).is_none());
    }

    #[test]
    fn a_different_or_missing_worker_endpoint_blocks_the_review() {
        let routes = [route(
            "opencode-go",
            Some(endpoint("https://opencode.ai/zen/go/v1")),
        )];
        let other_host = |_: &str| Some(endpoint("https://backup.example/zen/go/v1"));
        assert_eq!(
            provider_endpoint_mismatch(&routes, other_host).map(|route| route.provider.as_str()),
            Some("opencode-go")
        );
        let unconfigured = |_: &str| None;
        assert!(provider_endpoint_mismatch(&routes, unconfigured).is_some());
    }

    #[test]
    fn a_configured_worker_refuses_an_unpinned_scheduler_endpoint() {
        let routes = [route("opencode-go", None)];
        let configured = |_: &str| Some(endpoint("https://backup.example/zen/go/v1"));
        assert!(provider_endpoint_mismatch(&routes, configured).is_some());
        assert!(provider_endpoint_mismatch(&routes, |_| None).is_none());
    }

    #[test]
    fn the_first_mismatching_route_is_reported() {
        let routes = [
            route(
                "opencode-go",
                Some(endpoint("https://opencode.ai/zen/go/v1")),
            ),
            route("zai", Some(endpoint("https://api.z.ai/api/paas/v4"))),
        ];
        let worker = |provider: &str| match provider {
            "opencode-go" => Some(endpoint("https://opencode.ai/zen/go/v1")),
            _ => Some(endpoint("https://elsewhere.example/v4")),
        };
        let mismatch = provider_endpoint_mismatch(&routes, worker);
        assert_eq!(mismatch.map(|route| route.provider.as_str()), Some("zai"));
    }
}
