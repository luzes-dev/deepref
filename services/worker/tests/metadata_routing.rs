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
use deepref_application::jobs::ClaimedJob;
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
            max_depth: 0,
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
    sqlx::query("INSERT INTO jobs(id,project_id,kind,state,attempts,lease_owner,leased_until) VALUES($1,$2,'work_fetch_requested','running',1,$3,now()+interval '10 minutes')")
        .bind(event.event_id)
        .bind(project_id)
        .bind(OWNER)
        .execute(pool)
        .await?;
    let job = ClaimedJob {
        id: event.event_id,
        project_id: ProjectId::new(project_id),
        kind: JobKind::WorkFetchRequested,
        payload: serde_json::to_value(&event)?,
        attempts: 1,
        max_attempts: 5,
    };
    Ok(Fixture { project_id, job })
}

async fn run_fetch(pool: &PgPool, fixture: &Fixture, config: &WorkerConfig) -> Result<()> {
    let action = handle_job_with_documents_owned(
        pool.clone(),
        &fixture.job,
        &fixture.job.lease(OWNER),
        Duration::from_secs(30),
        None,
        None,
        config,
    )
    .await?;
    anyhow::ensure!(action == DeliveryAction::Ack, "the fetch should be acked");
    Ok(())
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
    let crossref = MockServer::start(Vec::new()).await;
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
    assert_eq!(saved.citation_provider, "openalex");
    assert_eq!(saved.cache_provider, "openalex");
    assert_eq!(saved.cache_status, "fetched");
    assert_eq!(
        saved.raw["deepref_provider_attempts"],
        json!([{ "provider": "openalex", "outcome": "answered" }])
    );
    assert_eq!(
        record_sources(&pool, fixture.project_id).await?,
        ["crossref"],
        "the DOI lane label does not change with the answering provider"
    );
    assert!(
        crossref.paths().is_empty(),
        "Crossref is not asked once OpenAlex answers"
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
    assert_eq!(saved.citation_provider, "crossref");
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
    assert_eq!(saved.citation_provider, "crossref");
    assert_eq!(saved.cache_provider, "crossref");
    assert_eq!(
        saved.raw["deepref_provider_attempts"],
        json!([
            { "provider": "openalex", "outcome": "not_found" },
            { "provider": "crossref", "outcome": "answered" },
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
