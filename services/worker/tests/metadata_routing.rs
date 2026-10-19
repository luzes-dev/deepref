#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::panic_in_result_fn,
    clippy::unwrap_used,
    clippy::string_slice
)]
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::Result;
use deepref_application::jobs::{ClaimedJob, JobScope};
use deepref_config::OpenAlexCorpus;
use deepref_domain::events::{
    EntityType, EventEnvelope, SUBJECT_WORK_FETCH_REQUESTED, WorkFetchRequested,
};
use deepref_domain::{JobKind, ProjectId};
use deepref_integrations::OpenAlexClient;
use deepref_worker::{
    config::WorkerConfig, delivery::DeliveryAction, metadata::MetadataClients,
    processor::handle_job_with_documents_owned,
};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

const CONTACT_EMAIL: &str = "ops@example.org";
const OWNER: &str = "worker-metadata-routing";

/// A scripted HTTP server. Each request is answered by the route whose path, without the
/// query, equals the request path. Unknown paths get a 404. Request lines are recorded in
/// arrival order.
struct MockServer {
    base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl MockServer {
    async fn start(routes: Vec<(String, &'static str, String)>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock server should bind");
        let base_url = format!(
            "http://{}",
            listener.local_addr().expect("mock server address")
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buffer = vec![0_u8; 16 * 1024];
                let read = socket.read(&mut buffer).await.unwrap_or(0);
                let request_line = String::from_utf8_lossy(&buffer[..read])
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                let path = request_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .split('?')
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                log.lock().expect("request log").push(request_line);
                let (status, body) = match routes.iter().find(|(route, _, _)| *route == path) {
                    Some((_, status, body)) => (*status, body.clone()),
                    None => ("404 Not Found", "{}".to_owned()),
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        Self { base_url, requests }
    }

    /// The request paths received so far, query strings removed, in arrival order.
    fn paths(&self) -> Vec<String> {
        self.requests
            .lock()
            .expect("request log")
            .iter()
            .map(|line| {
                line.split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .split('?')
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    }
}

fn openalex_work(doi: &str) -> String {
    json!({
        "id": "https://openalex.org/W1",
        "doi": format!("https://doi.org/{doi}"),
        "display_name": "Answered by OpenAlex",
        "publication_year": 2024,
        "cited_by_count": 3,
        "referenced_works": [],
        "referenced_works_count": 0,
        "abstract_inverted_index": { "Hello": [0], "world": [1] }
    })
    .to_string()
}

fn crossref_work() -> String {
    json!({
        "message": {
            "title": ["Answered by Crossref"],
            "is-referenced-by-count": 1,
            "references-count": 0
        }
    })
    .to_string()
}

/// A DataCite singleton body for `doi`, in the shape `GET /dois/{doi}?enriched=true` returns.
fn datacite_record(doi: &str) -> String {
    json!({
        "data": {
            "id": doi,
            "type": "dois",
            "attributes": {
                "doi": doi,
                "titles": [{ "title": "Answered by DataCite" }],
                "publisher": "Zenodo",
                "publicationYear": 2024,
                "types": { "resourceTypeGeneral": "Dataset" },
                "relatedIdentifiers": []
            }
        }
    })
    .to_string()
}

/// The worker configuration with every adapter pointed at the mock servers. The
/// abstract fallback is off, so no test reaches Europe PMC, the public OpenAlex API or
/// the public DataCite API.
fn worker_config(
    openalex: &MockServer,
    crossref: &MockServer,
    datacite: &MockServer,
) -> WorkerConfig {
    let mut config = WorkerConfig::from_source(&|name| match name {
        "APP_ENV" => Some("local".into()),
        "DEEPREF_CONTACT_EMAIL" => Some(CONTACT_EMAIL.into()),
        "DEEPREF_ABSTRACT_ENRICHMENT" => Some("off".into()),
        _ => None,
    })
    .expect("local worker configuration");
    config.metadata = MetadataClients {
        crossref_base_url: crossref.base_url.clone(),
        datacite_base_url: datacite.base_url.clone(),
        openalex: Some(
            OpenAlexClient::with_base_url(
                openalex.base_url.clone(),
                CONTACT_EMAIL,
                None,
                OpenAlexCorpus::Core,
            )
            .expect("OpenAlex client should build"),
        ),
    };
    config
}

struct Fixture {
    project_id: Uuid,
    job: ClaimedJob,
}

/// One queued DOI item under a running run whose stored config is `run_config`, with
/// its work-fetch job claimed by `OWNER`.
async fn seed_work_fetch(pool: &PgPool, doi: &str, run_config: Value) -> Result<Fixture> {
    seed_work_fetch_to_depth(pool, doi, run_config, 0).await
}

/// Like `seed_work_fetch`, for a run that traverses `max_depth` levels, so a cached
/// attachment replays the stored edges.
async fn seed_work_fetch_to_depth(
    pool: &PgPool,
    doi: &str,
    run_config: Value,
    max_depth: i32,
) -> Result<Fixture> {
    let project_id = Uuid::new_v4();
    let acquisition_id = Uuid::new_v4();
    let report_id = Uuid::new_v4();
    let event = EventEnvelope::v1(
        SUBJECT_WORK_FETCH_REQUESTED,
        "test",
        EntityType::Work,
        format!("{acquisition_id}|{doi}"),
        0,
        acquisition_id,
        None,
        WorkFetchRequested {
            project_id,
            acquisition_id,
            doi: doi.to_owned(),
            depth: 0,
            max_depth,
            parent_doi: None,
        },
    );
    sqlx::query("INSERT INTO projects(id,name) VALUES($1,'metadata routing fixture')")
        .bind(project_id)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO reports(id) VALUES($1)")
        .bind(report_id)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO acquisition_runs(id,project_id,source,strategy,status,seed_count,queued_count,created_at,config) VALUES($1,$2,'crossref','citation_traversal','running',1,1,now(),$3)")
        .bind(acquisition_id)
        .bind(project_id)
        .bind(run_config)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO acquisition_items(acquisition_id,project_id,report_id,canonical_doi,depth,work_event_id) VALUES($1,$2,$3,$4,0,$5)")
        .bind(acquisition_id)
        .bind(project_id)
        .bind(report_id)
        .bind(doi)
        .bind(event.event_id)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO jobs(id,project_id,kind,workload_class,state,attempts,lease_owner,leased_until) VALUES($1,$2,'work_fetch_requested','integration','running',1,$3,now()+interval '10 minutes')")
        .bind(event.event_id)
        .bind(project_id)
        .bind(OWNER)
        .execute(pool)
        .await?;
    let job = ClaimedJob {
        id: event.event_id,
        scope: JobScope::Project(ProjectId::new(project_id)),
        kind: JobKind::WorkFetchRequested,
        payload: serde_json::to_value(&event)?,
        attempts: 1,
        max_attempts: 5,
    };
    Ok(Fixture { project_id, job })
}

async fn run_fetch(pool: &PgPool, fixture: &Fixture, config: &WorkerConfig) -> Result<()> {
    let action = run_fetch_action(pool, fixture, config).await?;
    anyhow::ensure!(action == DeliveryAction::Ack, "the fetch should be acked");
    Ok(())
}

async fn run_fetch_action(
    pool: &PgPool,
    fixture: &Fixture,
    config: &WorkerConfig,
) -> Result<DeliveryAction> {
    handle_job_with_documents_owned(
        pool.clone(),
        &fixture.job,
        &fixture.job.lease(OWNER),
        Duration::from_secs(30),
        None,
        None,
        config,
    )
    .await
}

/// The work row's provenance, its raw payload, and the DOI's cache row provider and status.
struct Provenance {
    metadata_provider: String,
    citation_provider: String,
    raw: Value,
    cache_provider: String,
    cache_status: String,
}

async fn provenance(pool: &PgPool, doi: &str) -> Result<Provenance> {
    let work = sqlx::query(
        "SELECT metadata_provider, citation_provider, raw FROM works WHERE canonical_doi=$1",
    )
    .bind(doi)
    .fetch_one(pool)
    .await?;
    let cache = sqlx::query(
        "SELECT f.provider, f.status FROM report_fetch_state f JOIN report_identifiers i ON i.report_id=f.report_id WHERE i.scheme='doi' AND i.normalized_value=$1",
    )
    .bind(doi)
    .fetch_one(pool)
    .await?;
    Ok(Provenance {
        metadata_provider: work.get("metadata_provider"),
        citation_provider: work.get("citation_provider"),
        raw: work.get("raw"),
        cache_provider: cache.get("provider"),
        cache_status: cache.get("status"),
    })
}

async fn unresolved_facts(pool: &PgPool, doi: &str) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM fetched_unresolved_reference_facts WHERE source_doi=$1",
    )
    .bind(doi)
    .fetch_one(pool)
    .await?)
}

async fn unresolved_in_project(pool: &PgPool, project_id: Uuid) -> Result<i64> {
    Ok(
        sqlx::query_scalar(
            "SELECT count(*) FROM project_unresolved_references WHERE project_id=$1",
        )
        .bind(project_id)
        .fetch_one(pool)
        .await?,
    )
}

/// The lane labels of a project's citations, ordered by target DOI.
async fn citation_labels(pool: &PgPool, project_id: Uuid) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT c.source FROM citations c JOIN report_identifiers t ON t.report_id=c.target_report_id AND t.scheme='doi' WHERE c.project_id=$1 ORDER BY t.normalized_value",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await?)
}

/// The lane labels of the cached citation facts of one source DOI, ordered by target DOI.
async fn fact_labels(pool: &PgPool, doi: &str) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT source FROM fetched_citation_facts WHERE source_doi=$1 ORDER BY target_doi",
    )
    .bind(doi)
    .fetch_all(pool)
    .await?)
}

async fn record_sources(pool: &PgPool, project_id: Uuid) -> Result<Vec<String>> {
    Ok(
        sqlx::query_scalar("SELECT source FROM records WHERE project_id=$1")
            .bind(project_id)
            .fetch_all(pool)
            .await?,
    )
}

#[tokio::test]
async fn an_uncached_doi_is_answered_by_openalex_and_keeps_the_crossref_lane_label() -> Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
    else {
        return Ok(());
    };
    let doi = "10.5555/openalex-first";
    let openalex = MockServer::start(vec![(
        "/works/doi:10.5555/openalex-first".to_owned(),
        "200 OK",
        openalex_work(doi),
    )])
    .await;
    // OpenAlex lists no references, so only Crossref's list can show what it missed.
    let crossref = MockServer::start(vec![(
        "/works/10.5555%2Fopenalex-first".to_owned(),
        "200 OK",
        json!({"message": {"reference": [
            {"key": "r1", "DOI": "10.5555/cited"},
            {"key": "r2", "unstructured": "Smith 1999, a book with no DOI"}
        ]}})
        .to_string(),
    )])
    .await;
    let fixture = seed_work_fetch(&pool, doi, json!({})).await?;

    let datacite = MockServer::start(Vec::new()).await;
    run_fetch(
        &pool,
        &fixture,
        &worker_config(&openalex, &crossref, &datacite),
    )
    .await?;

    let saved = provenance(&pool, doi).await?;
    assert_eq!(saved.metadata_provider, "openalex");
    assert_eq!(
        saved.citation_provider, "crossref",
        "OpenAlex listed no references, so only Crossref contributed"
    );
    assert_eq!(saved.cache_provider, "openalex");
    assert_eq!(saved.raw["deepref_reference_status"]["status"], "complete");
    assert_eq!(saved.cache_status, "fetched");
    assert_eq!(
        saved.raw["deepref_provider_attempts"],
        json!([
            { "provider": "openalex", "purpose": "metadata", "requests": 1, "outcome": "answered" },
            { "provider": "crossref", "purpose": "reference_union", "requests": 1, "outcome": "answered" },
        ])
    );
    // The DOI-less entry reaches both unresolved-reference tables, and the cited DOI becomes
    // an edge with the provider-neutral label.
    assert_eq!(unresolved_facts(&pool, doi).await?, 1);
    assert_eq!(unresolved_in_project(&pool, fixture.project_id).await?, 1);
    assert_eq!(
        citation_labels(&pool, fixture.project_id).await?,
        ["doi-reference"]
    );
    assert_eq!(fact_labels(&pool, doi).await?, ["doi-reference"]);
    assert_eq!(
        record_sources(&pool, fixture.project_id).await?,
        ["crossref"],
        "the DOI lane label does not change with the answering provider"
    );
    assert_eq!(
        crossref.paths(),
        ["/works/10.5555%2Fopenalex-first"],
        "Crossref is asked only for the reference list once OpenAlex answers"
    );
    Ok(())
}

#[tokio::test]
async fn the_run_chain_wins_over_the_deployment_default_for_an_uncached_doi() -> Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
    else {
        return Ok(());
    };
    let doi = "10.5555/run-chain";
    // OpenAlex knows this DOI, so a fetch that used the deployment default would stop there.
    let openalex = MockServer::start(vec![(
        "/works/doi:10.5555/run-chain".to_owned(),
        "200 OK",
        openalex_work(doi),
    )])
    .await;
    let crossref = MockServer::start(vec![(
        "/works/10.5555%2Frun-chain".to_owned(),
        "200 OK",
        crossref_work(),
    )])
    .await;
    let fixture =
        seed_work_fetch(&pool, doi, json!({ "metadata_providers": ["crossref"] })).await?;

    let datacite = MockServer::start(Vec::new()).await;
    run_fetch(
        &pool,
        &fixture,
        &worker_config(&openalex, &crossref, &datacite),
    )
    .await?;

    let saved = provenance(&pool, doi).await?;
    assert_eq!(saved.metadata_provider, "crossref");
    assert_eq!(saved.citation_provider, "", "Crossref listed no references");
    assert_eq!(saved.cache_provider, "crossref");
    assert!(
        openalex.paths().is_empty(),
        "the stored chain names only Crossref, so OpenAlex is never asked"
    );
    assert_eq!(crossref.paths(), ["/works/10.5555%2Frun-chain"]);
    Ok(())
}

#[tokio::test]
async fn a_doi_openalex_does_not_know_falls_through_to_crossref() -> Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
    else {
        return Ok(());
    };
    let doi = "10.5555/openalex-miss";
    let openalex = MockServer::start(Vec::new()).await;
    let crossref = MockServer::start(vec![(
        "/works/10.5555%2Fopenalex-miss".to_owned(),
        "200 OK",
        crossref_work(),
    )])
    .await;
    let fixture = seed_work_fetch(&pool, doi, json!({})).await?;

    let datacite = MockServer::start(Vec::new()).await;
    run_fetch(
        &pool,
        &fixture,
        &worker_config(&openalex, &crossref, &datacite),
    )
    .await?;

    let saved = provenance(&pool, doi).await?;
    assert_eq!(saved.metadata_provider, "crossref");
    assert_eq!(saved.citation_provider, "", "Crossref listed no references");
    assert_eq!(saved.cache_provider, "crossref");
    assert_eq!(
        saved.raw["deepref_provider_attempts"],
        json!([
            { "provider": "openalex", "purpose": "metadata", "requests": 1, "outcome": "not_found" },
            { "provider": "crossref", "purpose": "metadata", "requests": 1, "outcome": "answered" },
        ])
    );
    assert_eq!(
        openalex.paths(),
        ["/works/doi:10.5555/openalex-miss"],
        "OpenAlex is asked first"
    );
    assert_eq!(
        crossref.paths(),
        ["/works/10.5555%2Fopenalex-miss"],
        "Crossref is asked once OpenAlex has no record"
    );
    Ok(())
}

#[tokio::test]
async fn a_paratext_work_ends_the_item_as_not_found_with_its_reason_and_no_later_provider_is_asked()
-> Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
    else {
        return Ok(());
    };
    let doi = "10.5555/paratext";
    let mut front_matter: Value = serde_json::from_str(&openalex_work(doi))?;
    front_matter["type"] = json!("paratext");
    let openalex = MockServer::start(vec![(
        "/works/doi:10.5555/paratext".to_owned(),
        "200 OK",
        front_matter.to_string(),
    )])
    .await;
    // Crossref would answer this DOI, so a fall-through would show up as a Crossref work.
    let crossref = MockServer::start(vec![(
        "/works/10.5555%2Fparatext".to_owned(),
        "200 OK",
        crossref_work(),
    )])
    .await;
    let fixture = seed_work_fetch(&pool, doi, json!({})).await?;

    let datacite = MockServer::start(Vec::new()).await;
    run_fetch(
        &pool,
        &fixture,
        &worker_config(&openalex, &crossref, &datacite),
    )
    .await?;

    let item =
        sqlx::query("SELECT status, last_error FROM acquisition_items WHERE canonical_doi=$1")
            .bind(doi)
            .fetch_one(&pool)
            .await?;
    assert_eq!(item.get::<String, _>("status"), "not_found");
    assert_eq!(
        item.get::<Option<String>, _>("last_error").as_deref(),
        Some("paratext")
    );
    let state = sqlx::query(
        "SELECT f.status, f.last_error FROM report_fetch_state f JOIN report_identifiers i ON i.report_id=f.report_id WHERE i.scheme='doi' AND i.normalized_value=$1",
    )
    .bind(doi)
    .fetch_one(&pool)
    .await?;
    assert_eq!(state.get::<String, _>("status"), "not_found");
    assert_eq!(
        state.get::<Option<String>, _>("last_error").as_deref(),
        Some("paratext")
    );
    // The claim leaves a placeholder row with only the DOI. Front matter never reaches
    // persist_work, so the title stays empty.
    let filed_title: Option<String> =
        sqlx::query_scalar("SELECT title FROM works WHERE canonical_doi=$1")
            .bind(doi)
            .fetch_one(&pool)
            .await?;
    assert_eq!(filed_title, None, "front matter is not filed as a work");
    assert_eq!(openalex.paths(), ["/works/doi:10.5555/paratext"]);
    assert!(
        crossref.paths().is_empty(),
        "front matter ends the chain at OpenAlex, so Crossref is never asked"
    );
    Ok(())
}

/// An OpenAlex answer for `doi` that names one referenced work and resolves nothing, so the
/// list endpoint's answer decides what the fetch finds.
fn openalex_citing_one(doi: &str) -> String {
    let mut work: Value = serde_json::from_str(&openalex_work(doi)).expect("fixture is JSON");
    work["referenced_works"] = json!(["https://openalex.org/W9"]);
    work["referenced_works_count"] = json!(1);
    work.to_string()
}

fn crossref_with_references(references: Value) -> String {
    json!({"message": {"title": ["Answered by Crossref"], "reference": references}}).to_string()
}

fn crossref_route(doi: &str, status: &'static str, body: String) -> (String, &'static str, String) {
    (format!("/works/{}", doi.replace('/', "%2F")), status, body)
}

#[tokio::test]
async fn the_union_runs_for_a_run_chain_of_openalex_alone_and_never_asks_crossref_for_metadata()
-> Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
    else {
        return Ok(());
    };
    let doi = "10.5555/openalex-only-chain";
    let openalex = MockServer::start(vec![(
        format!("/works/doi:{doi}"),
        "200 OK",
        openalex_work(doi),
    )])
    .await;
    let crossref = MockServer::start(vec![crossref_route(
        doi,
        "200 OK",
        crossref_with_references(json!([{ "DOI": "10.5555/cited" }])),
    )])
    .await;
    let fixture =
        seed_work_fetch(&pool, doi, json!({ "metadata_providers": ["openalex"] })).await?;

    let datacite = MockServer::start(Vec::new()).await;
    run_fetch(
        &pool,
        &fixture,
        &worker_config(&openalex, &crossref, &datacite),
    )
    .await?;

    let saved = provenance(&pool, doi).await?;
    assert_eq!(saved.metadata_provider, "openalex");
    assert_eq!(saved.citation_provider, "crossref");
    let purposes: Vec<(&str, &str)> = saved.raw["deepref_provider_attempts"]
        .as_array()
        .expect("attempts are an array")
        .iter()
        .map(|attempt| {
            (
                attempt["provider"].as_str().expect("provider"),
                attempt["purpose"].as_str().expect("purpose"),
            )
        })
        .collect();
    assert_eq!(
        purposes,
        [("openalex", "metadata"), ("crossref", "reference_union")],
        "Crossref is asked for references, never for metadata"
    );
    assert_eq!(fact_labels(&pool, doi).await?, ["doi-reference"]);
    Ok(())
}

#[tokio::test]
async fn a_legacy_run_config_stays_on_crossref_even_when_openalex_could_answer() -> Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
    else {
        return Ok(());
    };
    let doi = "10.5555/legacy-run";
    let openalex = MockServer::start(vec![(
        format!("/works/doi:{doi}"),
        "200 OK",
        openalex_work(doi),
    )])
    .await;
    let crossref = MockServer::start(vec![crossref_route(doi, "200 OK", crossref_work())]).await;
    let fixture = seed_work_fetch(
        &pool,
        doi,
        json!({
            "seed_dois": [doi],
            "max_depth": 0,
            "metadata_provider": "crossref",
            "citation_provider": "crossref"
        }),
    )
    .await?;

    let datacite = MockServer::start(Vec::new()).await;
    run_fetch(
        &pool,
        &fixture,
        &worker_config(&openalex, &crossref, &datacite),
    )
    .await?;

    let saved = provenance(&pool, doi).await?;
    assert_eq!(saved.metadata_provider, "crossref");
    assert!(
        openalex.paths().is_empty(),
        "the legacy shape ran on Crossref alone, so OpenAlex makes no metadata request"
    );
    Ok(())
}

#[tokio::test]
async fn a_failed_second_reference_batch_retries_the_job_without_asking_crossref_or_caching()
-> Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
    else {
        return Ok(());
    };
    let doi = "10.5555/second-batch";
    let ids: Vec<String> = (0..101)
        .map(|index| format!("W{}", 7_000_000 + index))
        .collect();
    let mut work: Value = serde_json::from_str(&openalex_work(doi))?;
    work["referenced_works"] = json!(
        ids.iter()
            .map(|id| format!("https://openalex.org/{id}"))
            .collect::<Vec<_>>()
    );
    work["referenced_works_count"] = json!(101);
    // The list endpoint answers every batch with a 502, so the first batch exhausts its
    // retries. Crossref would answer if the chain moved on.
    let openalex = MockServer::start(vec![
        (format!("/works/doi:{doi}"), "200 OK", work.to_string()),
        ("/works".to_owned(), "502 Bad Gateway", "{}".to_owned()),
    ])
    .await;
    let crossref = MockServer::start(vec![crossref_route(doi, "200 OK", crossref_work())]).await;
    let fixture = seed_work_fetch(&pool, doi, json!({})).await?;

    let datacite = MockServer::start(Vec::new()).await;
    let action = run_fetch_action(
        &pool,
        &fixture,
        &worker_config(&openalex, &crossref, &datacite),
    )
    .await?;

    assert!(matches!(action, DeliveryAction::Nak(_)), "{action:?}");
    assert!(
        crossref.paths().is_empty(),
        "a reference failure never selects another metadata provider"
    );
    let status: String = sqlx::query_scalar(
        "SELECT f.status FROM report_fetch_state f JOIN report_identifiers i ON i.report_id=f.report_id WHERE i.scheme='doi' AND i.normalized_value=$1",
    )
    .bind(doi)
    .fetch_one(&pool)
    .await?;
    assert_eq!(status, "failed", "no success cache entry");
    let title: Option<String> =
        sqlx::query_scalar("SELECT title FROM works WHERE canonical_doi=$1")
            .bind(doi)
            .fetch_one(&pool)
            .await?;
    assert_eq!(title, None, "no partial work is persisted");
    Ok(())
}

#[tokio::test]
async fn a_fatal_crossref_helper_failure_caches_a_degraded_success_with_its_reason() -> Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
    else {
        return Ok(());
    };
    let doi = "10.5555/helper-fatal";
    let openalex = MockServer::start(vec![
        (
            format!("/works/doi:{doi}"),
            "200 OK",
            openalex_citing_one(doi),
        ),
        (
            "/works".to_owned(),
            "200 OK",
            json!({"results": [{"id": "https://openalex.org/W9", "doi": "https://doi.org/10.5555/resolved"}]})
                .to_string(),
        ),
    ])
    .await;
    let crossref = MockServer::start(vec![crossref_route(
        doi,
        "200 OK",
        "this is not JSON".to_owned(),
    )])
    .await;
    let fixture = seed_work_fetch(&pool, doi, json!({})).await?;

    let datacite = MockServer::start(Vec::new()).await;
    run_fetch(
        &pool,
        &fixture,
        &worker_config(&openalex, &crossref, &datacite),
    )
    .await?;

    let saved = provenance(&pool, doi).await?;
    assert_eq!(saved.cache_status, "fetched");
    assert_eq!(saved.citation_provider, "openalex");
    assert_eq!(saved.raw["deepref_reference_status"]["status"], "degraded");
    assert_eq!(
        saved.raw["deepref_reference_status"]["reasons"],
        json!(["helper_fatal"])
    );
    assert_eq!(fact_labels(&pool, doi).await?, ["doi-reference"]);
    Ok(())
}

#[tokio::test]
async fn a_fatal_openalex_failure_is_recorded_and_the_chain_moves_on_to_crossref() -> Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
    else {
        return Ok(());
    };
    let doi = "10.5555/openalex-403";
    let openalex = MockServer::start(vec![(
        format!("/works/doi:{doi}"),
        "403 Forbidden",
        "{}".to_owned(),
    )])
    .await;
    let crossref = MockServer::start(vec![crossref_route(doi, "200 OK", crossref_work())]).await;
    let fixture = seed_work_fetch(&pool, doi, json!({})).await?;

    let datacite = MockServer::start(Vec::new()).await;
    run_fetch(
        &pool,
        &fixture,
        &worker_config(&openalex, &crossref, &datacite),
    )
    .await?;

    let saved = provenance(&pool, doi).await?;
    assert_eq!(saved.metadata_provider, "crossref");
    assert_eq!(
        saved.raw["deepref_provider_attempts"][0]["outcome"],
        "fatal"
    );
    Ok(())
}

#[tokio::test]
async fn when_every_provider_fails_fatally_the_item_fails_with_a_bounded_history() -> Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
    else {
        return Ok(());
    };
    let doi = "10.5555/all-fatal";
    let openalex = MockServer::start(vec![(
        format!("/works/doi:{doi}"),
        "403 Forbidden",
        "{}".to_owned(),
    )])
    .await;
    let crossref = MockServer::start(vec![crossref_route(
        doi,
        "400 Bad Request",
        "{}".to_owned(),
    )])
    .await;
    let datacite = MockServer::start(vec![(
        format!("/dois/{doi}"),
        "400 Bad Request",
        "{}".to_owned(),
    )])
    .await;
    let fixture = seed_work_fetch(&pool, doi, json!({})).await?;

    run_fetch(
        &pool,
        &fixture,
        &worker_config(&openalex, &crossref, &datacite),
    )
    .await?;

    let item =
        sqlx::query("SELECT status, last_error FROM acquisition_items WHERE canonical_doi=$1")
            .bind(doi)
            .fetch_one(&pool)
            .await?;
    assert_eq!(item.get::<String, _>("status"), "failed");
    let last_error = item
        .get::<Option<String>, _>("last_error")
        .unwrap_or_default();
    assert!(
        last_error.contains("openalex metadata fatal x1")
            && last_error.contains("crossref metadata fatal x1")
            && last_error.contains("datacite metadata fatal x1"),
        "{last_error}"
    );
    assert!(last_error.chars().count() <= 500, "{last_error}");
    let provider: String = sqlx::query_scalar(
        "SELECT f.provider FROM report_fetch_state f JOIN report_identifiers i ON i.report_id=f.report_id WHERE i.scheme='doi' AND i.normalized_value=$1",
    )
    .bind(doi)
    .fetch_one(&pool)
    .await?;
    assert_eq!(provider, "datacite", "the last provider attempted");
    Ok(())
}

#[tokio::test]
async fn a_cached_work_replays_its_edges_labels_and_unresolved_references_into_a_second_project()
-> Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
    else {
        return Ok(());
    };
    let doi = "10.5555/replayed";
    let openalex = MockServer::start(vec![(
        format!("/works/doi:{doi}"),
        "200 OK",
        openalex_work(doi),
    )])
    .await;
    let crossref = MockServer::start(vec![crossref_route(
        doi,
        "200 OK",
        crossref_with_references(json!([
            { "DOI": "10.5555/a-new-edge" },
            { "DOI": "10.5555/z-old-edge" },
            { "unstructured": "Smith 1999, a book with no DOI" }
        ])),
    )])
    .await;
    let datacite = MockServer::start(Vec::new()).await;
    let config = worker_config(&openalex, &crossref, &datacite);

    let first = seed_work_fetch_to_depth(&pool, doi, json!({}), 1).await?;
    run_fetch(&pool, &first, &config).await?;
    // A cache that holds an edge written before routing next to one written now.
    sqlx::query(
        "UPDATE fetched_citation_facts SET source='crossref-reference' WHERE source_doi=$1 AND target_doi='10.5555/z-old-edge'",
    )
    .bind(doi)
    .execute(&pool)
    .await?;

    let second = seed_work_fetch_to_depth(&pool, doi, json!({}), 1).await?;
    run_fetch(&pool, &second, &config).await?;

    assert_eq!(
        crossref.paths().len(),
        1,
        "the second project attaches the cache and fetches nothing"
    );
    assert_eq!(
        citation_labels(&pool, second.project_id).await?,
        ["doi-reference", "crossref-reference"],
        "each cached fact's label is carried into the project citation"
    );
    assert_eq!(unresolved_in_project(&pool, second.project_id).await?, 1);
    let saved = provenance(&pool, doi).await?;
    assert_eq!(saved.raw["deepref_reference_status"]["status"], "complete");
    Ok(())
}

#[tokio::test]
async fn a_doi_only_datacite_holds_is_recorded_as_datacite_and_keeps_the_crossref_lane_label()
-> Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
    else {
        return Ok(());
    };
    let doi = "10.5555/datacite-only";
    let openalex = MockServer::start(Vec::new()).await;
    let crossref = MockServer::start(Vec::new()).await;
    let datacite = MockServer::start(vec![(
        "/dois/10.5555/datacite-only".to_owned(),
        "200 OK",
        datacite_record(doi),
    )])
    .await;
    let fixture = seed_work_fetch(&pool, doi, json!({})).await?;

    run_fetch(
        &pool,
        &fixture,
        &worker_config(&openalex, &crossref, &datacite),
    )
    .await?;

    let saved = provenance(&pool, doi).await?;
    assert_eq!(saved.metadata_provider, "datacite");
    assert_eq!(saved.citation_provider, "");
    assert_eq!(saved.cache_provider, "datacite");
    assert_eq!(saved.cache_status, "fetched");
    assert_eq!(
        saved.raw["deepref_provider_attempts"],
        json!([
            { "provider": "openalex", "purpose": "metadata", "requests": 1, "outcome": "not_found" },
            { "provider": "crossref", "purpose": "metadata", "requests": 1, "outcome": "not_found" },
            { "provider": "datacite", "purpose": "metadata", "requests": 1, "outcome": "answered" },
        ])
    );
    assert_eq!(
        record_sources(&pool, fixture.project_id).await?,
        ["crossref"],
        "the DOI lane label does not change with the answering provider"
    );
    assert_eq!(openalex.paths(), ["/works/doi:10.5555/datacite-only"]);
    assert_eq!(crossref.paths(), ["/works/10.5555%2Fdatacite-only"]);
    assert_eq!(datacite.paths(), ["/dois/10.5555/datacite-only"]);
    Ok(())
}

#[tokio::test]
async fn a_datacite_only_edge_is_labelled_doi_reference_and_the_work_keeps_datacite_provenance()
-> Result<()> {
    let Some(pool) = deepref_app::test_isolation::isolated_pool_from_env("DATABASE_URL", 8).await
    else {
        return Ok(());
    };
    let doi = "10.5555/datacite-edge";
    let mut record: Value = serde_json::from_str(&datacite_record(doi))?;
    record["data"]["attributes"]["relatedIdentifiers"] = json!([
        { "relatedIdentifier": "10.5555/cited", "relatedIdentifierType": "DOI", "relationType": "Cites" },
        { "relatedIdentifier": "10.5555/citing", "relatedIdentifierType": "DOI", "relationType": "IsCitedBy" }
    ]);
    let openalex = MockServer::start(Vec::new()).await;
    let crossref = MockServer::start(Vec::new()).await;
    let datacite =
        MockServer::start(vec![(format!("/dois/{doi}"), "200 OK", record.to_string())]).await;
    let fixture = seed_work_fetch(&pool, doi, json!({})).await?;

    run_fetch(
        &pool,
        &fixture,
        &worker_config(&openalex, &crossref, &datacite),
    )
    .await?;

    let saved = provenance(&pool, doi).await?;
    assert_eq!(saved.metadata_provider, "datacite");
    assert_eq!(saved.citation_provider, "datacite");
    assert_eq!(
        citation_labels(&pool, fixture.project_id).await?,
        ["doi-reference"],
        "only the outbound Cites relation is an edge, and its label names no provider"
    );
    assert_eq!(fact_labels(&pool, doi).await?, ["doi-reference"]);
    Ok(())
}
