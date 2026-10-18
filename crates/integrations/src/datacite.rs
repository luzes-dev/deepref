//! DataCite: dataset, software and repository records by DOI. Singleton lookups are free
//! and need no key. The contact address travels in the User-Agent as `mailto:`.

mod wire;

use std::time::Duration;

use deepref_application::{
    FetchedWork, PermitError, ProviderFetchError, ProviderFuture, ProviderPermit, RawAuthor,
    RawIdentifier, RawRecord, WorkProvider,
};
use deepref_domain::{IdentifierScheme, ProviderKind, WorkWithReferences, normalize_doi};
use reqwest::{StatusCode, header};
use serde_json::Value;
use thiserror::Error;

use crate::crossref::sleep_backoff;
use wire::DataCiteAttributes;

pub const DATACITE_BASE: &str = "https://api.datacite.org";

/// Requests per second for the permit limiter (handoff section 5.1). DataCite allows
/// 1000 requests per five minutes per IP, about 3.3 per second.
pub const DATACITE_RATE_PER_SECOND: u32 = 3;

#[derive(Debug, Clone)]
pub struct DataCiteClient {
    client: reqwest::Client,
    base_url: String,
    max_attempts: usize,
}

impl DataCiteClient {
    /// A client for `base_url` that identifies the deployment by `mailto`. A blank
    /// address is refused, because every request sends it.
    pub fn with_base_url(
        base_url: impl Into<String>,
        mailto: impl Into<String>,
    ) -> Result<Self, DataCiteError> {
        let mailto = mailto.into();
        if mailto.trim().is_empty() {
            return Err(DataCiteError::MissingContactEmail);
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(format!("deepref/0.1 (mailto:{mailto})"))
            .build()?;
        Ok(Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            max_attempts: 5,
        })
    }

    #[must_use]
    pub fn with_max_attempts(mut self, max_attempts: usize) -> Self {
        self.max_attempts = max_attempts.max(1);
        self
    }

    /// The lookup under the adapter's retry policy. The caller's permit covers the
    /// first request, and each retry takes its own.
    async fn get_metered(
        &self,
        doi: &str,
        permit: &dyn ProviderPermit,
    ) -> Result<Value, DataCiteError> {
        let mut attempts = 0_usize;
        loop {
            if attempts > 0 {
                permit.acquire(ProviderKind::DataCite).await?;
            }
            attempts += 1;
            match self.get_once(doi).await {
                Err(error) if error.is_transient() && attempts < self.max_attempts => {
                    sleep_backoff(attempts).await;
                }
                outcome => return outcome,
            }
        }
    }

    /// One request. A 404 is not found, a 429 or 5xx is transient, and any other
    /// failure status is final.
    async fn get_once(&self, doi: &str) -> Result<Value, DataCiteError> {
        let encoded = urlencoding::encode(doi).replace("%2F", "/");
        let response = self
            .client
            .get(format!("{}/dois/{encoded}?enriched=true", self.base_url))
            .header(header::ACCEPT, "application/json")
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(status_error(status, doi));
        }
        let body = response.bytes().await?;
        Ok(serde_json::from_slice(&body)?)
    }

    /// The lookup and its identity check. The caller's permit covers the first request.
    async fn fetch_metered(
        &self,
        doi: &str,
        permit: &dyn ProviderPermit,
    ) -> Result<FetchedWork, DataCiteError> {
        let doi = normalize_doi(doi)?;
        let body = self.get_metered(&doi, permit).await?;
        Ok(FetchedWork {
            work: checked_work(&doi, body)?,
            provider: ProviderKind::DataCite,
            reference_gap: None,
        })
    }
}

fn status_error(status: StatusCode, doi: &str) -> DataCiteError {
    match status {
        StatusCode::NOT_FOUND => DataCiteError::NotFound(doi.to_owned()),
        StatusCode::TOO_MANY_REQUESTS => DataCiteError::RetryableStatus(status),
        _ if status.is_server_error() => DataCiteError::RetryableStatus(status),
        _ => DataCiteError::NonRetryableStatus(status),
    }
}

/// Maps a 200 body after the identity check. A record filed under another DOI, or with
/// no DOI, is not the work that was asked for.
fn checked_work(doi: &str, body: Value) -> Result<WorkWithReferences, DataCiteError> {
    let attributes = body
        .get("data")
        .and_then(|data| data.get("attributes"))
        .cloned()
        .unwrap_or(Value::Null);
    let typed: DataCiteAttributes = serde_json::from_value(attributes.clone())?;
    let returned = typed.doi.clone();
    let same_work = returned
        .as_deref()
        .and_then(|value| normalize_doi(value).ok())
        .as_deref()
        == Some(doi);
    if !same_work {
        tracing::warn!(
            requested = doi,
            returned = ?returned,
            "DataCite answered a different DOI; treating it as not found"
        );
        return Err(DataCiteError::DoiMismatch {
            requested: doi.to_owned(),
            returned,
        });
    }
    Ok(typed.into_domain(doi.to_owned(), attributes))
}

impl WorkProvider for DataCiteClient {
    fn kind(&self) -> ProviderKind {
        ProviderKind::DataCite
    }

    fn fetch_work_with_references<'a>(
        &'a self,
        doi: &'a str,
        permit: &'a dyn ProviderPermit,
    ) -> ProviderFuture<'a, FetchedWork, ProviderFetchError> {
        Box::pin(async move {
            self.fetch_metered(doi, permit)
                .await
                .map_err(DataCiteError::into_fetch_error)
        })
    }
}

/// The import-path record for a DataCite work, mirroring `raw_record_from_openalex_work`.
/// Authors come from the raw creators.
#[must_use]
pub fn raw_record_from_datacite_work(work: WorkWithReferences) -> RawRecord {
    let doi = work.work.doi.clone();
    let authors = work
        .raw
        .get("creators")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(raw_author)
        .collect();
    RawRecord {
        source_identifiers: vec![RawIdentifier {
            scheme: IdentifierScheme::Doi,
            value: doi.clone(),
            normalized_value: normalize_doi(&doi).unwrap_or(doi),
        }],
        title: work.work.title,
        abstract_text: work.work.abstract_text,
        authors,
        publication_year: work.work.published_year.or(work.work.issued_year),
        journal: work.work.container_title,
        raw: work.raw,
    }
}

/// A creator with a given or family name is named. Otherwise its name is kept as
/// written. A creator with neither is skipped.
fn raw_author(creator: &Value) -> Option<RawAuthor> {
    let text = |key: &str| {
        creator
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let given = text("givenName");
    let family = text("familyName");
    if given.is_some() || family.is_some() {
        return Some(RawAuthor::named(given, family));
    }
    text("name").map(RawAuthor::literal)
}

#[derive(Debug, Error)]
pub enum DataCiteError {
    #[error("a contact e-mail address is required; the server must set DEEPREF_CONTACT_EMAIL")]
    MissingContactEmail,
    #[error(transparent)]
    Permit(#[from] PermitError),
    #[error("DataCite returned not found for {0}")]
    NotFound(String),
    #[error("DataCite answered {returned:?} for a request for {requested}")]
    DoiMismatch {
        requested: String,
        returned: Option<String>,
    },
    #[error("DataCite returned retryable status: {0}")]
    RetryableStatus(StatusCode),
    #[error("DataCite returned non-retryable status: {0}")]
    NonRetryableStatus(StatusCode),
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("invalid DataCite response: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid DOI: {0}")]
    InvalidDoi(#[from] deepref_domain::DoiError),
}

impl DataCiteError {
    /// A failure a repeat of the same request can fix: a rate limit, a server error, or a
    /// network failure.
    fn is_transient(&self) -> bool {
        matches!(self, Self::RetryableStatus(_) | Self::Request(_))
    }

    /// Classifies a failed lookup for the provider chain.
    fn into_fetch_error(self) -> ProviderFetchError {
        let provider = ProviderKind::DataCite;
        let message = self.to_string();
        match self {
            Self::NotFound(doi) | Self::DoiMismatch { requested: doi, .. } => {
                ProviderFetchError::NotFound { provider, doi }
            }
            Self::InvalidDoi(_) => ProviderFetchError::Rejected {
                provider,
                reason: message,
            },
            Self::RetryableStatus(_) | Self::Request(_) | Self::Permit(_) => {
                ProviderFetchError::Retryable { provider, message }
            }
            Self::MissingContactEmail | Self::NonRetryableStatus(_) | Self::Json(_) => {
                ProviderFetchError::Fatal { provider, message }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    const SOFTWARE_DOI: &str = "10.5281/zenodo.22821143";
    const DATASET_DOI: &str = "10.5281/zenodo.22838416";
    /// A real Zenodo software record from `GET /dois/{doi}?enriched=true`, trimmed to the
    /// attributes the mapping reads.
    const SOFTWARE: &str =
        include_str!("../tests/fixtures/datacite/zenodo_software_citations.json");
    /// A real Zenodo dataset record from the DataCite list endpoint, trimmed. Its citation
    /// relations are all URLs.
    const DATASET: &str =
        include_str!("../tests/fixtures/datacite/zenodo_dataset_url_relations.json");
    /// A real Zenodo preprint from the list endpoint. It cites one DOI and is cited by one.
    const PREPRINT_DOI: &str = "10.5281/zenodo.20320323";
    const PREPRINT: &str = include_str!("../tests/fixtures/datacite/zenodo_preprint_cites.json");
    const ABSTRACT_START: &str =
        "Yatomi City (Aichi Prefecture, Japan) operates a demand-responsive transport service";

    /// One scripted response: status line, extra header lines, body.
    struct Reply {
        status: &'static str,
        body: String,
    }

    fn reply(status: &'static str, body: impl Into<String>) -> Reply {
        Reply {
            status,
            body: body.into(),
        }
    }

    /// Serves the scripted replies in order, one connection each, and records every
    /// request it receives in full, headers included.
    async fn scripted_server(replies: Vec<Reply>) -> (String, Arc<Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener.local_addr().expect("listener address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        tokio::spawn(async move {
            for reply in replies {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buffer = [0_u8; 8192];
                let read = socket.read(&mut buffer).await.unwrap_or(0);
                log.lock()
                    .expect("request log")
                    .push(String::from_utf8_lossy(&buffer[..read]).into_owned());
                let response = format!(
                    "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    reply.status,
                    reply.body.len(),
                    reply.body
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        (format!("http://{address}"), requests)
    }

    fn request_line(request: &str) -> &str {
        request.lines().next().unwrap_or_default()
    }

    #[derive(Default)]
    struct RecordingPermit(Mutex<Vec<ProviderKind>>);

    impl RecordingPermit {
        fn taken(&self) -> Vec<ProviderKind> {
            self.0.lock().expect("permit log").clone()
        }
    }

    impl ProviderPermit for RecordingPermit {
        fn acquire<'a>(&'a self, provider: ProviderKind) -> ProviderFuture<'a, (), PermitError> {
            Box::pin(async move {
                self.0.lock().expect("permit log").push(provider);
                Ok(())
            })
        }
    }

    fn client(base_url: String, max_attempts: usize) -> DataCiteClient {
        DataCiteClient::with_base_url(base_url, "ops@example.org")
            .expect("client should build")
            .with_max_attempts(max_attempts)
    }

    async fn fetch(
        client: &DataCiteClient,
        doi: &str,
        permit: &RecordingPermit,
    ) -> Result<FetchedWork, ProviderFetchError> {
        WorkProvider::fetch_work_with_references(client, doi, permit).await
    }

    fn reference_dois(fetched: &FetchedWork) -> Vec<&str> {
        fetched
            .work
            .references
            .iter()
            .filter_map(|reference| reference.doi.as_deref())
            .collect()
    }

    #[tokio::test]
    async fn a_found_record_is_answered_on_the_callers_permit_with_the_mapped_fields() {
        let (base, requests) = scripted_server(vec![reply("200 OK", SOFTWARE)]).await;
        let permit = RecordingPermit::default();

        let fetched = fetch(&client(base, 1), SOFTWARE_DOI, &permit)
            .await
            .expect("datacite should answer");

        assert_eq!(fetched.provider, ProviderKind::DataCite);
        assert_eq!(fetched.reference_gap, None);
        assert!(
            permit.taken().is_empty(),
            "the only request runs on the caller's permit"
        );
        let requests = requests.lock().expect("request log");
        assert_eq!(
            request_line(&requests[0]),
            "GET /dois/10.5281/zenodo.22821143?enriched=true HTTP/1.1"
        );
        assert!(
            requests[0]
                .to_ascii_lowercase()
                .contains("user-agent: deepref/0.1 (mailto:ops@example.org)"),
            "{}",
            requests[0]
        );
        let work = &fetched.work.work;
        assert_eq!(work.doi, SOFTWARE_DOI);
        assert_eq!(
            work.title.as_deref(),
            Some(
                "Building a GTFS-Flex Feed from Published Stop Coordinates: Demand Responsive Transport in Yatomi City, Aichi, Japan"
            )
        );
        assert_eq!(work.work_type.as_deref(), Some("Software"));
        assert_eq!(work.publisher.as_deref(), Some("Zenodo"));
        assert_eq!(
            work.container_title, None,
            "an empty container has no title"
        );
        assert_eq!(work.published_year, Some(2026));
        assert_eq!(work.issued_year, Some(2026));
        assert_eq!(
            work.url.as_deref(),
            Some("https://zenodo.org/doi/10.5281/zenodo.22821143")
        );
        assert_eq!(work.total_citations, 2);
        assert_eq!(work.references_count, 1);
        assert_eq!(work.metadata_provider, "datacite");
        assert_eq!(work.citation_provider, "datacite");
        assert_eq!(work.fetch_status, "fetched");
        assert_eq!(fetched.work.raw["rightsList"][0]["rightsIdentifier"], "mit");
        assert_eq!(
            fetched.work.raw["subjects"].as_array().map(Vec::len),
            Some(6)
        );
        assert!(
            fetched.work.raw.get("xml").is_none(),
            "the base64 XML repeats the JSON attributes and stays out of the payload"
        );
    }

    #[tokio::test]
    async fn the_abstract_comes_from_the_abstract_description() {
        let (base, _) = scripted_server(vec![reply("200 OK", SOFTWARE)]).await;

        let fetched = fetch(&client(base, 1), SOFTWARE_DOI, &RecordingPermit::default())
            .await
            .expect("datacite should answer");

        let abstract_text = fetched
            .work
            .work
            .abstract_text
            .as_deref()
            .expect("the Abstract description should map");
        assert!(abstract_text.starts_with(ABSTRACT_START), "{abstract_text}");
        assert!(!abstract_text.contains('<'), "{abstract_text}");
    }

    #[tokio::test]
    async fn cites_become_references_and_is_cited_by_stays_in_raw_on_a_second_record() {
        let (base, _) = scripted_server(vec![reply("200 OK", PREPRINT)]).await;

        let fetched = fetch(&client(base, 1), PREPRINT_DOI, &RecordingPermit::default())
            .await
            .expect("datacite should answer");

        assert_eq!(reference_dois(&fetched), ["10.5281/zenodo.20399676"]);
        assert_eq!(
            fetched.work.raw["deepref_datacite_incoming"],
            json!([{ "doi": "10.5281/zenodo.20412507", "relationType": "IsCitedBy" }])
        );
        assert!(
            !reference_dois(&fetched).contains(&"10.5281/zenodo.20412507"),
            "IsCitedBy names a citing work, never a reference"
        );
    }

    #[tokio::test]
    async fn references_become_references_and_incoming_relations_stay_in_raw() {
        let (base, _) = scripted_server(vec![reply("200 OK", SOFTWARE)]).await;

        let fetched = fetch(&client(base, 1), SOFTWARE_DOI, &RecordingPermit::default())
            .await
            .expect("datacite should answer");

        assert_eq!(reference_dois(&fetched), ["10.5281/zenodo.22608420"]);
        let references = reference_dois(&fetched);
        assert!(
            !references.contains(&"10.5281/zenodo.23442879"),
            "IsReferencedBy points at the citing work and is never a reference"
        );
        assert!(
            !references.contains(&"10.2139/ssrn.7511579"),
            "IsSupplementTo is not a citation relation"
        );
        assert!(
            !references.contains(&"10.5281/zenodo.22699096"),
            "IsVersionOf is not a citation relation"
        );
        assert_eq!(
            fetched.work.raw["deepref_datacite_incoming"],
            json!([{ "doi": "10.5281/zenodo.23442879", "relationType": "IsReferencedBy" }])
        );
    }

    #[tokio::test]
    async fn non_doi_relations_are_ignored_in_both_directions() {
        let (base, _) = scripted_server(vec![reply("200 OK", DATASET)]).await;

        let fetched = fetch(&client(base, 1), DATASET_DOI, &RecordingPermit::default())
            .await
            .expect("datacite should answer");

        assert!(
            fetched.work.references.is_empty(),
            "References to URLs cannot become child jobs"
        );
        assert_eq!(fetched.work.raw["deepref_datacite_incoming"], json!([]));
        assert!(
            fetched.work.raw["relatedIdentifiers"]
                .as_array()
                .is_some_and(|relations| relations
                    .iter()
                    .any(|relation| relation["relatedIdentifierType"] == "URL")),
            "the URL relations stay in the raw payload"
        );
    }

    #[tokio::test]
    async fn the_raw_record_carries_creators_with_orcids_and_the_work_as_an_import_would() {
        let (base, _) = scripted_server(vec![reply("200 OK", SOFTWARE)]).await;
        let fetched = fetch(&client(base, 1), SOFTWARE_DOI, &RecordingPermit::default())
            .await
            .expect("datacite should answer");

        let record = raw_record_from_datacite_work(fetched.work);

        assert_eq!(record.source_identifiers.len(), 1);
        assert_eq!(record.source_identifiers[0].scheme, IdentifierScheme::Doi);
        assert_eq!(record.source_identifiers[0].normalized_value, SOFTWARE_DOI);
        assert_eq!(
            record.authors,
            vec![RawAuthor::named(
                Some("Yasutoshi".to_owned()),
                Some("Moteki".to_owned())
            )]
        );
        assert_eq!(record.publication_year, Some(2026));
        assert_eq!(record.journal, None);
        assert!(
            record
                .abstract_text
                .as_deref()
                .is_some_and(|text| text.starts_with(ABSTRACT_START))
        );
        assert_eq!(
            record.raw["creators"][0]["nameIdentifiers"][0]["nameIdentifier"],
            "0000-0001-7763-670X"
        );
    }

    #[test]
    fn a_creator_is_named_when_it_has_a_name_part_and_literal_otherwise() {
        assert_eq!(
            raw_author(&json!({ "name": "Ghezelbash, Saeed", "familyName": "Ghezelbash" })),
            Some(RawAuthor::named(None, Some("Ghezelbash".to_owned())))
        );
        assert_eq!(
            raw_author(&json!({ "name": "Zenodo Community" })),
            Some(RawAuthor::literal("Zenodo Community"))
        );
        assert_eq!(raw_author(&json!({ "name": "  " })), None);
    }

    #[tokio::test]
    async fn a_missing_work_is_not_found_and_not_retried() {
        let (base, requests) = scripted_server(vec![reply("404 Not Found", "{}")]).await;
        let permit = RecordingPermit::default();

        let error = fetch(&client(base, 3), SOFTWARE_DOI, &permit)
            .await
            .expect_err("a 404 should not answer");

        assert_eq!(
            error,
            ProviderFetchError::NotFound {
                provider: ProviderKind::DataCite,
                doi: SOFTWARE_DOI.to_owned(),
            }
        );
        assert_eq!(requests.lock().expect("request log").len(), 1);
        assert!(permit.taken().is_empty());
    }

    #[tokio::test]
    async fn a_rate_limit_is_retried_on_a_new_permit_and_then_answers() {
        let (base, requests) = scripted_server(vec![
            reply("429 Too Many Requests", "{}"),
            reply("200 OK", SOFTWARE),
        ])
        .await;
        let permit = RecordingPermit::default();

        let fetched = fetch(&client(base, 2), SOFTWARE_DOI, &permit)
            .await
            .expect("the retry should answer");

        assert_eq!(fetched.provider, ProviderKind::DataCite);
        assert_eq!(
            permit.taken(),
            vec![ProviderKind::DataCite],
            "the retry takes its own permit"
        );
        assert_eq!(requests.lock().expect("request log").len(), 2);
    }

    #[tokio::test]
    async fn a_server_error_that_persists_is_retryable_after_every_attempt() {
        let (base, requests) = scripted_server(vec![
            reply("503 Service Unavailable", "{}"),
            reply("503 Service Unavailable", "{}"),
        ])
        .await;

        let error = fetch(&client(base, 2), SOFTWARE_DOI, &RecordingPermit::default())
            .await
            .expect_err("a persistent 503 should not answer");

        match error {
            ProviderFetchError::Retryable { provider, message } => {
                assert_eq!(provider, ProviderKind::DataCite);
                assert!(message.contains("503"), "{message}");
            }
            other => panic!("expected a retryable failure, got {other:?}"),
        }
        assert_eq!(requests.lock().expect("request log").len(), 2);
    }

    #[tokio::test]
    async fn other_client_errors_are_fatal_without_a_retry() {
        for status in ["400 Bad Request", "403 Forbidden", "408 Request Timeout"] {
            let (base, requests) = scripted_server(vec![reply(status, "{}")]).await;

            let error = fetch(&client(base, 3), SOFTWARE_DOI, &RecordingPermit::default())
                .await
                .expect_err("a client error should not answer");

            assert!(
                matches!(
                    error,
                    ProviderFetchError::Fatal {
                        provider: ProviderKind::DataCite,
                        ..
                    }
                ),
                "{status}: {error:?}"
            );
            assert_eq!(requests.lock().expect("request log").len(), 1, "{status}");
        }
    }

    #[tokio::test]
    async fn a_connection_failure_is_retryable() {
        let error = fetch(
            &client("http://127.0.0.1:9".to_owned(), 2),
            SOFTWARE_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect_err("nothing listens on the discard port");

        assert!(
            matches!(
                error,
                ProviderFetchError::Retryable {
                    provider: ProviderKind::DataCite,
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn a_record_filed_under_another_doi_is_not_found() {
        let (base, _) = scripted_server(vec![reply("200 OK", DATASET)]).await;

        let error = fetch(&client(base, 1), SOFTWARE_DOI, &RecordingPermit::default())
            .await
            .expect_err("a sibling record is not the requested work");

        assert_eq!(
            error,
            ProviderFetchError::NotFound {
                provider: ProviderKind::DataCite,
                doi: SOFTWARE_DOI.to_owned(),
            }
        );
    }

    #[tokio::test]
    async fn a_permit_is_taken_before_every_request() {
        let (base, requests) = scripted_server(vec![
            reply("429 Too Many Requests", "{}"),
            reply("503 Service Unavailable", "{}"),
            reply("200 OK", SOFTWARE),
        ])
        .await;
        let permit = RecordingPermit::default();
        // The router takes the permit for the first request before it calls the adapter.
        permit
            .acquire(ProviderKind::DataCite)
            .await
            .expect("the caller's permit");

        fetch(&client(base, 3), SOFTWARE_DOI, &permit)
            .await
            .expect("the third attempt should answer");

        let sent = requests.lock().expect("request log").len();
        assert_eq!(sent, 3);
        assert_eq!(
            permit.taken().len(),
            sent,
            "one permit per request, the first one taken by the caller"
        );
    }

    #[test]
    fn a_blank_contact_address_refuses_the_client() {
        assert!(matches!(
            DataCiteClient::with_base_url(DATACITE_BASE, "  "),
            Err(DataCiteError::MissingContactEmail)
        ));
    }

    #[test]
    fn each_fetch_failure_maps_to_the_outcome_the_chain_acts_on() {
        let provider = ProviderKind::DataCite;
        let invalid = normalize_doi("not a doi").expect_err("not a DOI");
        let json = serde_json::from_str::<u32>("x").expect_err("not a number");

        assert_eq!(
            DataCiteError::NotFound(SOFTWARE_DOI.to_owned()).into_fetch_error(),
            ProviderFetchError::NotFound {
                provider,
                doi: SOFTWARE_DOI.to_owned(),
            }
        );
        assert_eq!(
            DataCiteError::DoiMismatch {
                requested: SOFTWARE_DOI.to_owned(),
                returned: None,
            }
            .into_fetch_error(),
            ProviderFetchError::NotFound {
                provider,
                doi: SOFTWARE_DOI.to_owned(),
            }
        );
        assert!(matches!(
            DataCiteError::InvalidDoi(invalid).into_fetch_error(),
            ProviderFetchError::Rejected { .. }
        ));
        assert!(matches!(
            DataCiteError::RetryableStatus(StatusCode::TOO_MANY_REQUESTS).into_fetch_error(),
            ProviderFetchError::Retryable { .. }
        ));
        assert!(matches!(
            DataCiteError::Permit(PermitError("limiter is down".to_owned())).into_fetch_error(),
            ProviderFetchError::Retryable { .. }
        ));
        assert!(matches!(
            DataCiteError::NonRetryableStatus(StatusCode::FORBIDDEN).into_fetch_error(),
            ProviderFetchError::Fatal { .. }
        ));
        assert!(matches!(
            DataCiteError::Json(json).into_fetch_error(),
            ProviderFetchError::Fatal { .. }
        ));
        assert!(matches!(
            DataCiteError::MissingContactEmail.into_fetch_error(),
            ProviderFetchError::Fatal { .. }
        ));
    }
}
