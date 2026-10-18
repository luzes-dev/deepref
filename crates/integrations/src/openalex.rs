//! OpenAlex: work metadata by DOI. Singleton lookups are free, so the key only
//! lifts the per-second rate. A key that fails downgrades the process to keyless
//! calls until restart.

mod wire;

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use deepref_application::{
    FetchedWork, PermitError, ProviderFetchError, ProviderFuture, ProviderPermit, WorkProvider,
};
use deepref_config::{IntegrationsConfig, OpenAlexCorpus};
use deepref_domain::{ProviderKind, WorkWithReferences, normalize_doi};
use reqwest::{StatusCode, header};
use serde_json::Value;
use thiserror::Error;

use crate::crossref::{StatusClassification, classify_status, sleep_backoff};
use wire::OpenAlexWork;

const OPENALEX_BASE: &str = "https://api.openalex.org";
/// The fields the mapping reads, plus the ones kept in the raw payload for
/// enrichment. Anything else in a work stays out of the response.
const SELECT_FIELDS: &str = concat!(
    "id,doi,display_name,publication_year,publication_date,type,primary_location,",
    "cited_by_count,referenced_works,referenced_works_count,best_oa_location,locations,",
    "is_retracted,is_paratext,ids,topics,authorships,abstract_inverted_index"
);

/// Requests per second with a valid key (handoff section 5.1).
pub const KEYED_RATE_PER_SECOND: u32 = 25;
/// Requests per second once the key is absent or has been downgraded.
pub const KEYLESS_RATE_PER_SECOND: u32 = 8;

/// Whether this process sends the OpenAlex key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAlexKeyState {
    Keyed,
    Keyless,
}

/// The configured key and its one-way downgrade. The flag is atomic so every clone
/// of the client sees the same state without holding a lock across a request.
pub struct OpenAlexKey {
    key: Option<String>,
    downgraded: AtomicBool,
}

impl OpenAlexKey {
    /// A blank key counts as no key.
    pub fn new(key: Option<String>) -> Self {
        Self {
            key: key.filter(|key| !key.trim().is_empty()),
            downgraded: AtomicBool::new(false),
        }
    }

    pub fn state(&self) -> OpenAlexKeyState {
        if self.key.is_some() && !self.downgraded.load(Ordering::Acquire) {
            OpenAlexKeyState::Keyed
        } else {
            OpenAlexKeyState::Keyless
        }
    }

    pub fn rate_per_second(&self) -> u32 {
        match self.state() {
            OpenAlexKeyState::Keyed => KEYED_RATE_PER_SECOND,
            OpenAlexKeyState::Keyless => KEYLESS_RATE_PER_SECOND,
        }
    }

    /// The key to send, or `None` once the process is keyless.
    fn key(&self) -> Option<&str> {
        match self.state() {
            OpenAlexKeyState::Keyed => self.key.as_deref(),
            OpenAlexKeyState::Keyless => None,
        }
    }

    /// Moves a keyed process to keyless. Returns true only for the call that makes
    /// the change, and only that call logs the warning, so it fires once per process.
    pub fn downgrade(&self, reason: &'static str) -> bool {
        if self.key.is_none() {
            return false;
        }
        let first = !self.downgraded.swap(true, Ordering::AcqRel);
        if first {
            tracing::warn!(
                reason,
                rate_per_second = KEYLESS_RATE_PER_SECOND,
                "OpenAlex key unusable; continuing keyless until restart"
            );
        }
        first
    }
}

impl fmt::Debug for OpenAlexKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAlexKey")
            .field("key", &self.key.as_ref().map(|_| "<redacted>"))
            .field("state", &self.state())
            .finish()
    }
}

/// The OpenAlex client. The caller's permit covers the first request of a lookup.
/// Every later request takes its own permit from the callback, as the other
/// adapters do.
#[derive(Debug, Clone)]
pub struct OpenAlexClient {
    client: reqwest::Client,
    base_url: String,
    mailto: String,
    key: Arc<OpenAlexKey>,
    corpus: OpenAlexCorpus,
    max_attempts: usize,
}

impl OpenAlexClient {
    /// The client for `api.openalex.org`, configured from the deployment settings.
    pub fn new(config: &IntegrationsConfig) -> Result<Self, OpenAlexError> {
        Self::with_base_url(
            OPENALEX_BASE,
            config.contact_email.clone(),
            config.openalex_api_key.clone(),
            config.openalex_corpus,
        )
    }

    /// Builds a client for another OpenAlex-compatible base URL. A blank contact
    /// address is refused, as it is for Crossref, because every request sends it.
    pub fn with_base_url(
        base_url: impl Into<String>,
        mailto: impl Into<String>,
        api_key: Option<String>,
        corpus: OpenAlexCorpus,
    ) -> Result<Self, OpenAlexError> {
        let mailto = mailto.into();
        if mailto.trim().is_empty() {
            return Err(OpenAlexError::MissingContactEmail);
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(format!("deepref/0.1 (mailto:{mailto})"))
            .build()
            .map_err(transport)?;
        Ok(Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            mailto,
            key: Arc::new(OpenAlexKey::new(api_key)),
            corpus,
            max_attempts: 5,
        })
    }

    #[must_use]
    pub fn with_max_attempts(mut self, max_attempts: usize) -> Self {
        self.max_attempts = max_attempts.max(1);
        self
    }

    pub fn key_state(&self) -> OpenAlexKeyState {
        self.key.state()
    }

    /// The per-second rate for the permit limiter. It follows the key state, so a
    /// downgrade lowers it for every later call.
    pub fn rate_per_second(&self) -> u32 {
        self.key.rate_per_second()
    }

    /// Which OpenAlex records a DOI miss may fall back to (`corpus=all` lists).
    pub fn corpus(&self) -> OpenAlexCorpus {
        self.corpus
    }

    /// Asks `GET /rate-limit` whether the configured key works, once at boot. The
    /// call is free. Only a 401, or a 403 that says the key is not activated,
    /// downgrades the key. Any other outcome keeps it, and a key that fails later is
    /// downgraded by the first request it fails. Without a key there is no call.
    pub async fn probe_key(&self, permit: &dyn ProviderPermit) -> OpenAlexKeyState {
        let Some(key) = self.key.key() else {
            return OpenAlexKeyState::Keyless;
        };
        if let Err(error) = permit.acquire(ProviderKind::OpenAlex).await {
            tracing::warn!(%error, "OpenAlex key probe skipped; keeping the key");
            return self.key.state();
        }
        let url = format!(
            "{}/rate-limit?api_key={}&mailto={}",
            self.base_url,
            urlencoding::encode(key),
            urlencoding::encode(&self.mailto),
        );
        match self
            .client
            .get(url)
            .header(header::ACCEPT, "application/json")
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {}
            Ok(response) if response.status() == StatusCode::UNAUTHORIZED => {
                self.key.downgrade("boot probe returned 401");
            }
            Ok(response) if response.status() == StatusCode::FORBIDDEN => {
                let body = response.text().await.unwrap_or_default();
                if body.contains("key_not_activated") {
                    self.key.downgrade("boot probe found the key not activated");
                } else {
                    tracing::warn!(
                        status = 403,
                        "OpenAlex key probe refused; keeping the key until a request fails"
                    );
                }
            }
            Ok(response) => {
                tracing::warn!(
                    status = %response.status(),
                    "OpenAlex key probe was inconclusive; keeping the key"
                );
            }
            Err(error) => {
                let error = error.without_url();
                tracing::warn!(
                    %error,
                    "OpenAlex unreachable at boot; keeping the key until a request reaches it"
                );
            }
        }
        self.key.state()
    }

    /// `GET /works/doi:{doi}` with the selected fields. The key is sent as a query
    /// parameter only while the process is keyed.
    fn singleton_request(&self, doi: &str) -> reqwest::RequestBuilder {
        let mut url = format!(
            "{}/works/doi:{}?select={SELECT_FIELDS}&mailto={}",
            self.base_url,
            urlencoding::encode(doi).replace("%2F", "/"),
            urlencoding::encode(&self.mailto),
        );
        if let Some(key) = self.key.key() {
            url.push_str("&api_key=");
            url.push_str(&urlencoding::encode(key));
        }
        self.client
            .get(url)
            .header(header::ACCEPT, "application/json")
    }

    /// One lookup under the adapter's retry policy. A rejected key spends no attempt:
    /// the repeat is a new request without the key, not a retry of the same one.
    async fn fetch_metered(
        &self,
        doi: &str,
        permit: &dyn ProviderPermit,
    ) -> Result<WorkWithReferences, OpenAlexError> {
        let doi = normalize_doi(doi)?;
        let mut requests = 0_usize;
        let mut attempts = 0_usize;
        loop {
            if requests > 0 {
                permit.acquire(ProviderKind::OpenAlex).await?;
            }
            requests += 1;
            let sent_with_key = self.key.key().is_some();

            let failure = match self.singleton_request(&doi).send().await {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        let raw: Value = response.json().await.map_err(transport)?;
                        return checked_work(&doi, raw);
                    }
                    match status {
                        StatusCode::NOT_FOUND => return Err(OpenAlexError::NotFound(doi)),
                        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN if sent_with_key => {
                            self.key.downgrade("OpenAlex rejected the key");
                            continue;
                        }
                        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                            return Err(OpenAlexError::Unauthorized(status));
                        }
                        _ => match classify_status(status) {
                            StatusClassification::Retryable => {
                                OpenAlexError::RetryableStatus(status)
                            }
                            _ => return Err(OpenAlexError::NonRetryableStatus(status)),
                        },
                    }
                }
                Err(error) if error.is_timeout() || error.is_connect() => transport(error),
                Err(error) => return Err(transport(error)),
            };

            attempts += 1;
            if attempts >= self.max_attempts {
                return Err(failure);
            }
            sleep_backoff(attempts).await;
        }
    }
}

/// reqwest puts the request URL in its error text, and the URL carries the key.
fn transport(error: reqwest::Error) -> OpenAlexError {
    OpenAlexError::Request(error.without_url())
}

/// Maps a 200 body after the identity check. A sibling record filed under another
/// DOI, or a null DOI, is not the work that was asked for, so it is not found.
fn checked_work(doi: &str, raw: Value) -> Result<WorkWithReferences, OpenAlexError> {
    let body: OpenAlexWork = serde_json::from_value(raw.clone())?;
    let same_work = body
        .doi
        .as_deref()
        .and_then(|returned| normalize_doi(returned).ok())
        .as_deref()
        == Some(doi);
    if !same_work {
        tracing::warn!(
            requested = doi,
            returned = ?body.doi,
            "OpenAlex answered a different DOI; treating it as not found"
        );
        return Err(OpenAlexError::DoiMismatch {
            requested: doi.to_owned(),
            returned: body.doi,
        });
    }
    Ok(body.into_domain(doi.to_owned(), raw))
}

impl WorkProvider for OpenAlexClient {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenAlex
    }

    fn fetch_work_with_references<'a>(
        &'a self,
        doi: &'a str,
        permit: &'a dyn ProviderPermit,
    ) -> ProviderFuture<'a, FetchedWork, ProviderFetchError> {
        Box::pin(async move {
            let work = self
                .fetch_metered(doi, permit)
                .await
                .map_err(OpenAlexError::into_fetch_error)?;
            Ok(FetchedWork {
                work,
                provider: ProviderKind::OpenAlex,
            })
        })
    }
}

#[derive(Debug, Error)]
pub enum OpenAlexError {
    #[error("a contact e-mail address is required; the server must set DEEPREF_CONTACT_EMAIL")]
    MissingContactEmail,
    #[error(transparent)]
    Permit(#[from] PermitError),
    #[error("OpenAlex returned not found for {0}")]
    NotFound(String),
    #[error("OpenAlex answered {returned:?} for a request for {requested}")]
    DoiMismatch {
        requested: String,
        returned: Option<String>,
    },
    #[error("OpenAlex refused a keyless request: {0}")]
    Unauthorized(StatusCode),
    #[error("OpenAlex returned retryable status: {0}")]
    RetryableStatus(StatusCode),
    #[error("OpenAlex returned non-retryable status: {0}")]
    NonRetryableStatus(StatusCode),
    #[error("request failed: {0}")]
    Request(reqwest::Error),
    #[error("invalid OpenAlex response: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid DOI: {0}")]
    InvalidDoi(#[from] deepref_domain::DoiError),
}

impl OpenAlexError {
    /// Classifies a failed lookup for the provider chain. A keyless refusal is
    /// Retryable rather than Fatal: OpenAlex cannot answer now, so the chain moves on.
    fn into_fetch_error(self) -> ProviderFetchError {
        let provider = ProviderKind::OpenAlex;
        let message = self.to_string();
        match self {
            Self::NotFound(doi) | Self::DoiMismatch { requested: doi, .. } => {
                ProviderFetchError::NotFound { provider, doi }
            }
            Self::InvalidDoi(_) => ProviderFetchError::Rejected {
                provider,
                reason: message,
            },
            Self::Unauthorized(_)
            | Self::RetryableStatus(_)
            | Self::Request(_)
            | Self::Permit(_) => ProviderFetchError::Retryable { provider, message },
            Self::MissingContactEmail | Self::NonRetryableStatus(_) | Self::Json(_) => {
                ProviderFetchError::Fatal { provider, message }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const ABSTRACT_DOI: &str = "10.1371/journal.pmed.0020124";
    const SIBLING_REQUESTED_DOI: &str = "10.48550/arxiv.1706.03762";
    const NO_REFERENCES_DOI: &str = "10.5281/zenodo.1";
    const WORK: &str = include_str!("../tests/fixtures/openalex/work_with_abstract.json");
    const SIBLING: &str = include_str!("../tests/fixtures/openalex/work_doi_mismatch.json");
    const NO_REFERENCES: &str =
        include_str!("../tests/fixtures/openalex/work_without_references.json");
    const NOT_FOUND: &str = include_str!("../tests/fixtures/openalex/not_found.html");
    const INVALID_KEY: &str = include_str!("../tests/fixtures/openalex/key_invalid.json");

    /// One scripted response: status line, extra header lines, body.
    struct Reply {
        status: &'static str,
        headers: String,
        body: String,
    }

    fn reply(status: &'static str, body: impl Into<String>) -> Reply {
        Reply {
            status,
            headers: String::new(),
            body: body.into(),
        }
    }

    fn redirect(location: &str) -> Reply {
        Reply {
            status: "301 Moved Permanently",
            headers: format!("Location: {location}\r\n"),
            body: String::new(),
        }
    }

    /// Serves the scripted replies in order, one connection each, and records the
    /// request line of every connection it accepts.
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
                let request_line = String::from_utf8_lossy(&buffer[..read])
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                log.lock().expect("request log").push(request_line);
                let response = format!(
                    "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}",
                    reply.status,
                    reply.body.len(),
                    reply.headers,
                    reply.body
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        (format!("http://{address}"), requests)
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

    fn client(base_url: String, api_key: Option<&str>, max_attempts: usize) -> OpenAlexClient {
        OpenAlexClient::with_base_url(
            base_url,
            "ops@example.org",
            api_key.map(str::to_owned),
            OpenAlexCorpus::Core,
        )
        .expect("client should build")
        .with_max_attempts(max_attempts)
    }

    async fn fetch(
        client: &OpenAlexClient,
        doi: &str,
        permit: &RecordingPermit,
    ) -> Result<FetchedWork, ProviderFetchError> {
        WorkProvider::fetch_work_with_references(client, doi, permit).await
    }

    #[tokio::test]
    async fn a_found_work_is_answered_on_the_callers_permit_with_the_selected_fields() {
        let (base, requests) = scripted_server(vec![reply("200 OK", WORK)]).await;
        let client = client(base, None, 1);
        let permit = RecordingPermit::default();

        let fetched = fetch(&client, ABSTRACT_DOI, &permit)
            .await
            .expect("openalex should answer");

        assert_eq!(client.kind(), ProviderKind::OpenAlex);
        assert_eq!(fetched.provider, ProviderKind::OpenAlex);
        assert_eq!(
            fetched.work.work.title.as_deref(),
            Some("Why Most Published Research Findings Are False")
        );
        assert!(
            permit.taken().is_empty(),
            "the first request runs on the permit the caller took"
        );
        let requests = requests.lock().expect("request log");
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].starts_with("GET /works/doi:10.1371/journal.pmed.0020124?select="),
            "{}",
            requests[0]
        );
        assert!(
            requests[0].contains("&mailto=ops%40example.org"),
            "{}",
            requests[0]
        );
        assert!(!requests[0].contains("api_key"), "{}", requests[0]);
    }

    #[tokio::test]
    async fn the_mapping_carries_the_fixture_fields_and_rebuilds_the_abstract() {
        let (base, _) = scripted_server(vec![reply("200 OK", WORK)]).await;
        let fetched = fetch(
            &client(base, None, 1),
            ABSTRACT_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect("openalex should answer");
        let work = &fetched.work;

        assert_eq!(work.work.doi, ABSTRACT_DOI);
        assert_eq!(work.work.work_type.as_deref(), Some("article"));
        assert_eq!(work.work.published_year, Some(2005));
        assert_eq!(work.work.issued_year, Some(2005));
        assert_eq!(work.work.container_title.as_deref(), Some("PLoS Medicine"));
        assert_eq!(
            work.work.publisher.as_deref(),
            Some("Public Library of Science")
        );
        assert_eq!(work.work.total_citations, 10451);
        assert_eq!(work.work.references_count, 40);
        assert_eq!(work.work.metadata_provider, "openalex");
        assert_eq!(work.work.citation_provider, "openalex");
        assert_eq!(work.work.fetch_status, "fetched");
        assert!(
            work.work
                .abstract_text
                .as_deref()
                .is_some_and(|text| text.starts_with(
                    "There is increasing concern that most current published research findings are false."
                )),
            "{:?}",
            work.work.abstract_text
        );
        assert_eq!(work.raw["is_retracted"], Value::Bool(false));
        assert_eq!(work.raw["is_paratext"], Value::Bool(false));
        assert_eq!(work.raw["topics"].as_array().map(Vec::len), Some(2));
        assert_eq!(
            work.raw["authorships"][0]["author"]["orcid"],
            "https://orcid.org/0000-0003-3118-6859"
        );
        assert_eq!(
            work.raw["deepref_oa_urls"]["locations"]
                .as_array()
                .map(Vec::len),
            Some(2)
        );
        assert!(work.raw["deepref_oa_urls"]["best_oa_location"].is_object());
    }

    #[tokio::test]
    async fn references_stay_empty_and_the_openalex_ids_wait_in_raw_for_the_resolver() {
        let (base, _) = scripted_server(vec![reply("200 OK", WORK)]).await;
        let fetched = fetch(
            &client(base, None, 1),
            ABSTRACT_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect("openalex should answer");

        assert!(fetched.work.references.is_empty());
        let pending = fetched.work.raw["deepref_openalex_referenced_works"]
            .as_array()
            .expect("the pending list is an array");
        assert_eq!(pending.len(), 40);
        assert_eq!(
            pending,
            fetched.work.raw["referenced_works"].as_array().unwrap()
        );
    }

    #[tokio::test]
    async fn a_work_without_references_keeps_an_empty_list_and_missing_source_fields_stay_none() {
        let (base, _) = scripted_server(vec![reply("200 OK", NO_REFERENCES)]).await;
        let fetched = fetch(
            &client(base, None, 1),
            NO_REFERENCES_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect("openalex should answer");

        assert_eq!(fetched.work.work.references_count, 0);
        assert_eq!(
            fetched.work.raw["deepref_openalex_referenced_works"],
            serde_json::json!([])
        );
        assert_eq!(fetched.work.work.publisher, None);
        assert_eq!(
            fetched.work.work.container_title.as_deref(),
            Some("Journal of Child Development, Exceptionality and Education")
        );
        assert_eq!(fetched.work.work.published_year, Some(2025));
    }

    #[tokio::test]
    async fn a_redirect_is_followed_and_the_matching_work_answers() {
        let (base, requests) =
            scripted_server(vec![redirect("/works/W2144981148"), reply("200 OK", WORK)]).await;

        let fetched = fetch(
            &client(base, None, 1),
            ABSTRACT_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect("the redirected work should answer");

        assert_eq!(fetched.work.work.doi, ABSTRACT_DOI);
        assert_eq!(requests.lock().expect("request log").len(), 2);
    }

    #[tokio::test]
    async fn a_redirect_to_a_sibling_record_is_not_found() {
        let (base, _) = scripted_server(vec![
            redirect("/works/W2626778328"),
            reply("200 OK", SIBLING),
        ])
        .await;

        let error = fetch(
            &client(base, None, 1),
            SIBLING_REQUESTED_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect_err("a sibling record must not answer");

        assert_eq!(
            error,
            ProviderFetchError::NotFound {
                provider: ProviderKind::OpenAlex,
                doi: SIBLING_REQUESTED_DOI.to_owned(),
            }
        );
    }

    #[tokio::test]
    async fn a_sibling_record_filed_under_another_doi_is_not_answered_for_the_requested_one() {
        let (base, _) = scripted_server(vec![reply("200 OK", SIBLING)]).await;

        let error = fetch(
            &client(base, None, 1),
            SIBLING_REQUESTED_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect_err("the sibling record must not answer");

        assert!(matches!(
            error,
            ProviderFetchError::NotFound { provider: ProviderKind::OpenAlex, ref doi }
                if doi == SIBLING_REQUESTED_DOI
        ));
    }

    #[tokio::test]
    async fn a_null_doi_is_not_found() {
        let mut body: Value = serde_json::from_str(WORK).expect("fixture is JSON");
        body["doi"] = Value::Null;
        let (base, _) = scripted_server(vec![reply("200 OK", body.to_string())]).await;

        let error = fetch(
            &client(base, None, 1),
            ABSTRACT_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect_err("a work without a DOI must not answer");

        assert!(matches!(
            error,
            ProviderFetchError::NotFound {
                provider: ProviderKind::OpenAlex,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn a_missing_work_is_not_found() {
        let (base, _) = scripted_server(vec![reply("404 Not Found", NOT_FOUND)]).await;

        let error = fetch(
            &client(base, None, 1),
            ABSTRACT_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect_err("a 404 should not answer");

        assert_eq!(
            error,
            ProviderFetchError::NotFound {
                provider: ProviderKind::OpenAlex,
                doi: ABSTRACT_DOI.to_owned(),
            }
        );
    }

    #[tokio::test]
    async fn a_rate_limited_singleton_is_retried_on_a_new_permit_even_with_no_budget_left() {
        let (base, requests) = scripted_server(vec![
            Reply {
                status: "429 Too Many Requests",
                headers: "X-RateLimit-Remaining: 0\r\n".to_owned(),
                body: "{}".to_owned(),
            },
            reply("200 OK", WORK),
        ])
        .await;
        let permit = RecordingPermit::default();

        let fetched = fetch(&client(base, None, 2), ABSTRACT_DOI, &permit)
            .await
            .expect("the retry should answer");

        assert_eq!(fetched.provider, ProviderKind::OpenAlex);
        assert_eq!(permit.taken(), vec![ProviderKind::OpenAlex]);
        assert_eq!(requests.lock().expect("request log").len(), 2);
    }

    #[tokio::test]
    async fn a_rate_limit_on_a_singleton_is_retryable_when_no_attempt_is_left() {
        let (base, _) = scripted_server(vec![reply("429 Too Many Requests", "{}")]).await;

        let error = fetch(
            &client(base, None, 1),
            ABSTRACT_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect_err("the rate limit should not answer");

        match error {
            ProviderFetchError::Retryable { provider, message } => {
                assert_eq!(provider, ProviderKind::OpenAlex);
                assert!(message.contains("429"), "{message}");
            }
            other => panic!("expected a retryable failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_server_error_that_persists_is_retryable_after_every_attempt() {
        let (base, requests) = scripted_server(vec![
            reply("500 Internal Server Error", "{}"),
            reply("500 Internal Server Error", "{}"),
        ])
        .await;
        let permit = RecordingPermit::default();

        let error = fetch(&client(base, None, 2), ABSTRACT_DOI, &permit)
            .await
            .expect_err("a persistent 500 should not answer");

        match error {
            ProviderFetchError::Retryable { message, .. } => {
                assert!(message.contains("500"), "{message}");
            }
            other => panic!("expected a retryable failure, got {other:?}"),
        }
        assert_eq!(requests.lock().expect("request log").len(), 2);
        assert_eq!(permit.taken(), vec![ProviderKind::OpenAlex]);
    }

    /// A runtime rejection of the key: the first request carries it, the repeat
    /// does not, and every later lookup in the process is keyless from the start.
    async fn assert_rejected_key_downgrades_once(status: &'static str) {
        let (base, requests) = scripted_server(vec![
            reply(status, INVALID_KEY),
            reply("200 OK", WORK),
            reply("200 OK", WORK),
        ])
        .await;
        let client = client(base, Some("test-key"), 1);
        let permit = RecordingPermit::default();

        let fetched = fetch(&client, ABSTRACT_DOI, &permit)
            .await
            .expect("the keyless repeat should answer");

        assert_eq!(fetched.provider, ProviderKind::OpenAlex);
        assert_eq!(permit.taken(), vec![ProviderKind::OpenAlex]);
        assert_eq!(client.key_state(), OpenAlexKeyState::Keyless);
        assert_eq!(client.rate_per_second(), KEYLESS_RATE_PER_SECOND);

        let second = RecordingPermit::default();
        fetch(&client, ABSTRACT_DOI, &second)
            .await
            .expect("later lookups stay keyless and answer");
        assert!(second.taken().is_empty());

        let requests = requests.lock().expect("request log");
        assert_eq!(requests.len(), 3);
        assert!(requests[0].contains("api_key=test-key"), "{}", requests[0]);
        assert!(!requests[1].contains("api_key"), "{}", requests[1]);
        assert!(!requests[2].contains("api_key"), "{}", requests[2]);
    }

    #[tokio::test]
    async fn a_runtime_401_downgrades_the_key_once_and_the_repeat_is_keyless() {
        assert_rejected_key_downgrades_once("401 Unauthorized").await;
    }

    #[tokio::test]
    async fn a_runtime_403_downgrades_the_key_once_and_the_repeat_is_keyless() {
        assert_rejected_key_downgrades_once("403 Forbidden").await;
    }

    #[tokio::test]
    async fn a_keyless_refusal_is_retryable_so_the_chain_moves_on() {
        let (base, _) = scripted_server(vec![reply("403 Forbidden", INVALID_KEY)]).await;

        let error = fetch(
            &client(base, None, 1),
            ABSTRACT_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect_err("a refused keyless request should not answer");

        assert!(matches!(
            error,
            ProviderFetchError::Retryable {
                provider: ProviderKind::OpenAlex,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn an_invalid_doi_is_rejected_before_any_request() {
        let (base, requests) = scripted_server(Vec::new()).await;
        let permit = RecordingPermit::default();

        let error = fetch(&client(base, None, 1), "not a doi", &permit)
            .await
            .expect_err("an invalid DOI should not reach OpenAlex");

        assert!(matches!(
            error,
            ProviderFetchError::Rejected {
                provider: ProviderKind::OpenAlex,
                ..
            }
        ));
        assert!(requests.lock().expect("request log").is_empty());
        assert!(permit.taken().is_empty());
    }

    #[tokio::test]
    async fn a_connection_failure_is_retryable_and_never_echoes_the_key() {
        let client = client("http://127.0.0.1:1".to_owned(), Some("test-key"), 2);
        let permit = RecordingPermit::default();

        let error = fetch(&client, ABSTRACT_DOI, &permit)
            .await
            .expect_err("nothing listens on that port");

        match &error {
            ProviderFetchError::Retryable { provider, message } => {
                assert_eq!(*provider, ProviderKind::OpenAlex);
                assert!(!message.contains("test-key"), "{message}");
            }
            other => panic!("expected a retryable failure, got {other:?}"),
        }
        assert_eq!(permit.taken(), vec![ProviderKind::OpenAlex]);
    }

    #[tokio::test]
    async fn a_missing_key_is_keyless_and_the_probe_makes_no_call() {
        let (base, requests) = scripted_server(Vec::new()).await;
        let permit = RecordingPermit::default();

        let state = client(base, None, 1).probe_key(&permit).await;

        assert_eq!(state, OpenAlexKeyState::Keyless);
        assert!(requests.lock().expect("request log").is_empty());
        assert!(permit.taken().is_empty());
    }

    #[tokio::test]
    async fn an_accepted_key_stays_keyed_after_one_permitted_probe() {
        let (base, requests) = scripted_server(vec![reply("200 OK", "{}")]).await;
        let client = client(base, Some("test-key"), 1);
        let permit = RecordingPermit::default();

        let state = client.probe_key(&permit).await;

        assert_eq!(state, OpenAlexKeyState::Keyed);
        assert_eq!(client.rate_per_second(), KEYED_RATE_PER_SECOND);
        assert_eq!(permit.taken(), vec![ProviderKind::OpenAlex]);
        let requests = requests.lock().expect("request log");
        assert!(
            requests[0].starts_with("GET /rate-limit?api_key=test-key&mailto="),
            "{}",
            requests[0]
        );
    }

    #[tokio::test]
    async fn a_rejected_key_at_boot_is_downgraded_to_keyless() {
        let (base, _) = scripted_server(vec![reply("401 Unauthorized", INVALID_KEY)]).await;
        let client = client(base, Some("test-key"), 1);

        let state = client.probe_key(&RecordingPermit::default()).await;

        assert_eq!(state, OpenAlexKeyState::Keyless);
        assert_eq!(client.key_state(), OpenAlexKeyState::Keyless);
        assert_eq!(client.rate_per_second(), KEYLESS_RATE_PER_SECOND);
    }

    #[tokio::test]
    async fn a_key_not_yet_activated_at_boot_is_downgraded_to_keyless() {
        // No pending key exists to provoke the real 403, so the body follows the handoff.
        let (base, _) = scripted_server(vec![reply(
            "403 Forbidden",
            r#"{"error":"key_not_activated"}"#,
        )])
        .await;
        let client = client(base, Some("test-key"), 1);

        let state = client.probe_key(&RecordingPermit::default()).await;

        assert_eq!(state, OpenAlexKeyState::Keyless);
        assert_eq!(client.rate_per_second(), KEYLESS_RATE_PER_SECOND);
    }

    #[tokio::test]
    async fn a_403_without_the_activation_marker_keeps_the_key() {
        let (base, _) = scripted_server(vec![reply("403 Forbidden", "{}")]).await;

        let state = client(base, Some("test-key"), 1)
            .probe_key(&RecordingPermit::default())
            .await;

        assert_eq!(state, OpenAlexKeyState::Keyed);
    }

    #[tokio::test]
    async fn a_server_error_at_boot_keeps_the_key() {
        let (base, _) = scripted_server(vec![reply("500 Internal Server Error", "{}")]).await;

        let state = client(base, Some("test-key"), 1)
            .probe_key(&RecordingPermit::default())
            .await;

        assert_eq!(state, OpenAlexKeyState::Keyed);
    }

    #[tokio::test]
    async fn an_unreachable_openalex_at_boot_keeps_the_key() {
        let client = client("http://127.0.0.1:1".to_owned(), Some("test-key"), 1);
        let permit = RecordingPermit::default();

        let state = client.probe_key(&permit).await;

        assert_eq!(state, OpenAlexKeyState::Keyed);
        assert_eq!(permit.taken(), vec![ProviderKind::OpenAlex]);
    }

    #[test]
    fn the_client_debug_output_never_shows_the_key() {
        let client = client("http://127.0.0.1:1".to_owned(), Some("test-key"), 1);
        let debug = format!("{client:?}");
        assert!(!debug.contains("test-key"), "{debug}");
    }

    #[test]
    fn each_fetch_failure_maps_to_the_outcome_the_chain_acts_on() {
        let provider = ProviderKind::OpenAlex;
        let invalid = deepref_domain::normalize_doi("not a doi").expect_err("not a DOI");
        let json = serde_json::from_str::<u32>("x").expect_err("not a number");

        assert_eq!(
            OpenAlexError::NotFound(ABSTRACT_DOI.to_owned()).into_fetch_error(),
            ProviderFetchError::NotFound {
                provider,
                doi: ABSTRACT_DOI.to_owned(),
            }
        );
        assert_eq!(
            OpenAlexError::DoiMismatch {
                requested: SIBLING_REQUESTED_DOI.to_owned(),
                returned: Some("10.65215/2q58a426".to_owned()),
            }
            .into_fetch_error(),
            ProviderFetchError::NotFound {
                provider,
                doi: SIBLING_REQUESTED_DOI.to_owned(),
            }
        );
        assert!(matches!(
            OpenAlexError::InvalidDoi(invalid).into_fetch_error(),
            ProviderFetchError::Rejected { .. }
        ));
        assert!(matches!(
            OpenAlexError::RetryableStatus(StatusCode::BAD_GATEWAY).into_fetch_error(),
            ProviderFetchError::Retryable { .. }
        ));
        assert!(matches!(
            OpenAlexError::Unauthorized(StatusCode::FORBIDDEN).into_fetch_error(),
            ProviderFetchError::Retryable { .. }
        ));
        assert!(matches!(
            OpenAlexError::Permit(PermitError("limiter is down".to_owned())).into_fetch_error(),
            ProviderFetchError::Retryable { .. }
        ));
        assert!(matches!(
            OpenAlexError::NonRetryableStatus(StatusCode::BAD_REQUEST).into_fetch_error(),
            ProviderFetchError::Fatal { .. }
        ));
        assert!(matches!(
            OpenAlexError::Json(json).into_fetch_error(),
            ProviderFetchError::Fatal { .. }
        ));
        assert!(matches!(
            OpenAlexError::MissingContactEmail.into_fetch_error(),
            ProviderFetchError::Fatal { .. }
        ));
    }
}
