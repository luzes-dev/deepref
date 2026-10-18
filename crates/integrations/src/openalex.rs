//! OpenAlex: work metadata by DOI. Singleton lookups are free, so the key only
//! lifts the per-second rate. A key that fails downgrades the process to keyless
//! calls until restart.

mod wire;

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use chrono::{NaiveDate, Utc};
use deepref_application::{
    FetchedWork, PermitError, ProviderFetchError, ProviderFuture, ProviderPermit, RawAuthor,
    RawIdentifier, RawRecord, ReferenceGap, WorkProvider,
};
use deepref_config::{IntegrationsConfig, OpenAlexCorpus};
use deepref_domain::{
    IdentifierScheme, ProviderKind, Reference, WorkWithReferences, normalize_doi,
};
use reqwest::{StatusCode, header};
use serde_json::Value;
use thiserror::Error;

use crate::crossref::{StatusClassification, classify_status, sleep_backoff};
use wire::OpenAlexWork;

const OPENALEX_BASE: &str = "https://api.openalex.org";
/// The fields the mapping reads, plus the ones kept in the raw payload for
/// enrichment. Anything else in a work stays out of the response. Funding is `awards`
/// and `funders` here: the API rejects `grants` as a select field.
const SELECT_FIELDS: &str = concat!(
    "id,doi,display_name,publication_year,publication_date,type,primary_location,",
    "cited_by_count,referenced_works,referenced_works_count,best_oa_location,locations,",
    "is_retracted,is_paratext,ids,topics,authorships,abstract_inverted_index,",
    "counts_by_year,awards,funders"
);

/// IDs per reference list call. One `ids.openalex:` filter takes at most 100 values,
/// and one page returns at most 100 results.
const LIST_CHUNK: usize = 100;

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

/// What this process knows about the daily list budget. A list call that comes back 429
/// with no budget left stops every later list call until the reset. The low-budget
/// warning fires once per UTC day.
#[derive(Debug, Default)]
struct ListBudget {
    stop: Mutex<Option<Stop>>,
    warned_on: Mutex<Option<NaiveDate>>,
}

#[derive(Debug, Clone, Copy)]
struct Stop {
    until: Instant,
    reset_secs: Option<u64>,
}

impl ListBudget {
    /// Stops list calls until the reset. Without a reset header the stop lasts until the
    /// next UTC midnight, when the daily budget refills.
    fn stop(&self, now: Instant, reset_secs: Option<u64>) {
        let window = reset_secs.unwrap_or_else(seconds_until_utc_midnight);
        *lock(&self.stop) = Some(Stop {
            until: now + Duration::from_secs(window),
            reset_secs,
        });
    }

    /// The reset reported when the stop began, while the stop still holds at `now`.
    fn stopped(&self, now: Instant) -> Option<Option<u64>> {
        let stop = *lock(&self.stop);
        stop.filter(|stop| now < stop.until)
            .map(|stop| stop.reset_secs)
    }

    /// True for the first low-budget response of each UTC day.
    fn claim_daily_warning(&self, today: NaiveDate) -> bool {
        let mut warned_on = lock(&self.warned_on);
        if *warned_on == Some(today) {
            return false;
        }
        *warned_on = Some(today);
        true
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A poisoned lock still holds the last value written, which is all these markers need.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether the remaining budget is under a tenth of the limit.
fn budget_is_low(limit: u64, remaining: u64) -> bool {
    remaining.saturating_mul(10) < limit
}

/// Seconds until the next UTC midnight, when the daily budget resets.
fn seconds_until_utc_midnight() -> u64 {
    const DAY: i64 = 86_400;
    (DAY - Utc::now().timestamp().rem_euclid(DAY)).unsigned_abs()
}

/// An integer rate-limit header, or `None` when it is absent or not a number.
fn header_u64(headers: &header::HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

/// The two request shapes. A singleton is free and answers on the caller's permit. A list
/// call costs money, so it takes a permit of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Endpoint {
    Singleton,
    List,
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
    budget: Arc<ListBudget>,
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
            budget: Arc::new(ListBudget::default()),
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

    /// Logs the low-budget warning once per UTC day, for a response that carries both the
    /// limit and the remaining budget and shows less than a tenth left.
    fn observe_budget(&self, headers: &header::HeaderMap) {
        let (Some(limit), Some(remaining)) = (
            header_u64(headers, "x-ratelimit-limit"),
            header_u64(headers, "x-ratelimit-remaining"),
        ) else {
            return;
        };
        if budget_is_low(limit, remaining)
            && self.budget.claim_daily_warning(Utc::now().date_naive())
        {
            tracing::warn!(
                limit,
                remaining,
                "OpenAlex daily budget is under 10 percent"
            );
        }
    }

    /// The request URL for a path and query. The key is added only while the process
    /// is keyed, so a keyless process never sends it.
    fn request_url(&self, path_and_query: &str) -> String {
        let mut url = format!(
            "{}{}&mailto={}",
            self.base_url,
            path_and_query,
            urlencoding::encode(&self.mailto),
        );
        if let Some(key) = self.key.key() {
            url.push_str("&api_key=");
            url.push_str(&urlencoding::encode(key));
        }
        url
    }

    /// One GET under the adapter's retry policy. `Ok(None)` is a 404. A rejected key
    /// spends no attempt: the repeat is a new request without the key, not a retry of
    /// the same one. Every request but the caller's first takes its own permit.
    async fn get_metered(
        &self,
        endpoint: Endpoint,
        path_and_query: &str,
        permit: &dyn ProviderPermit,
    ) -> Result<Option<Value>, OpenAlexError> {
        if endpoint == Endpoint::List
            && let Some(reset_secs) = self.budget.stopped(Instant::now())
        {
            return Err(OpenAlexError::BudgetExhausted { reset_secs });
        }
        let mut requests = 0_usize;
        let mut attempts = 0_usize;
        loop {
            if requests > 0 || endpoint == Endpoint::List {
                permit.acquire(ProviderKind::OpenAlex).await?;
            }
            requests += 1;
            let sent_with_key = self.key.key().is_some();
            let request = self
                .client
                .get(self.request_url(path_and_query))
                .header(header::ACCEPT, "application/json");

            let failure = match request.send().await {
                Ok(response) => {
                    let status = response.status();
                    self.observe_budget(response.headers());
                    if status.is_success() {
                        let raw: Value = response.json().await.map_err(transport)?;
                        return Ok(Some(raw));
                    }
                    match status {
                        StatusCode::NOT_FOUND => return Ok(None),
                        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN if sent_with_key => {
                            self.key.downgrade("OpenAlex rejected the key");
                            continue;
                        }
                        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                            return Err(OpenAlexError::Unauthorized(status));
                        }
                        StatusCode::TOO_MANY_REQUESTS
                            if endpoint == Endpoint::List
                                && header_u64(response.headers(), "x-ratelimit-remaining")
                                    == Some(0) =>
                        {
                            let reset_secs = header_u64(response.headers(), "x-ratelimit-reset");
                            self.budget.stop(Instant::now(), reset_secs);
                            tracing::warn!(
                                ?reset_secs,
                                "OpenAlex daily budget is spent; list calls stop until the reset"
                            );
                            return Err(OpenAlexError::BudgetExhausted { reset_secs });
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

    /// The singleton by DOI. Under `corpus=all`, a miss is retried once through a list
    /// call over the full corpus. A stopped budget or a miss there answers not found.
    async fn lookup_raw(
        &self,
        doi: &str,
        permit: &dyn ProviderPermit,
    ) -> Result<Option<Value>, OpenAlexError> {
        let encoded = urlencoding::encode(doi).replace("%2F", "/");
        let singleton = self
            .get_metered(
                Endpoint::Singleton,
                &format!("/works/doi:{encoded}?select={SELECT_FIELDS}"),
                permit,
            )
            .await?;
        if singleton.is_some() || self.corpus != OpenAlexCorpus::All {
            return Ok(singleton);
        }
        let path = format!("/works?filter=doi:{encoded}&corpus=all&select={SELECT_FIELDS}");
        match self.get_metered(Endpoint::List, &path, permit).await {
            Ok(Some(body)) => Ok(body.pointer("/results/0").cloned()),
            Ok(None) | Err(OpenAlexError::BudgetExhausted { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Looks the DOI up, then resolves its references. The caller's permit covers the
    /// singleton's first request.
    async fn fetch_metered(
        &self,
        doi: &str,
        permit: &dyn ProviderPermit,
    ) -> Result<FetchedWork, OpenAlexError> {
        let doi = normalize_doi(doi)?;
        let raw = self
            .lookup_raw(&doi, permit)
            .await?
            .ok_or_else(|| OpenAlexError::NotFound(doi.clone()))?;
        let mut work = checked_work(&doi, raw)?;
        let (references, budget_gap) = self
            .resolve_references(&wire::referenced_ids(&work.raw), permit)
            .await?;
        let reference_gap =
            budget_gap.or_else(|| shortfall(references.len(), work.work.references_count));
        work.references = references;
        Ok(FetchedWork {
            work,
            provider: ProviderKind::OpenAlex,
            reference_gap,
        })
    }

    /// Resolves OpenAlex reference IDs to DOIs, one list call per `LIST_CHUNK` IDs, run
    /// in order. Targets without a DOI are left out. A budget stop keeps the references
    /// resolved so far and reports the gap, and the metadata still answers.
    async fn resolve_references(
        &self,
        ids: &[String],
        permit: &dyn ProviderPermit,
    ) -> Result<(Vec<Reference>, Option<ReferenceGap>), OpenAlexError> {
        let mut dois = HashMap::new();
        let mut resolved = 0;
        for chunk in ids.chunks(LIST_CHUNK) {
            let path = format!(
                "/works?filter=ids.openalex:{}&select=id,doi&per_page={LIST_CHUNK}",
                chunk.join("|")
            );
            match self.get_metered(Endpoint::List, &path, permit).await {
                Ok(Some(body)) => {
                    dois.extend(wire::list_dois(&body));
                    resolved += chunk.len();
                }
                Ok(None) => {
                    return Err(OpenAlexError::NonRetryableStatus(StatusCode::NOT_FOUND));
                }
                Err(OpenAlexError::BudgetExhausted { reset_secs }) => {
                    let gap = ReferenceGap::BudgetExhausted {
                        resolved,
                        expected: ids.len(),
                        reset_secs,
                    };
                    return Ok((references_in_order(ids, &dois), Some(gap)));
                }
                Err(error) => return Err(error),
            }
        }
        Ok((references_in_order(ids, &dois), None))
    }
}

/// The gap for an answer that lists fewer DOI references than the work reports. A target
/// without a DOI at OpenAlex is missing from the list too, so it counts here.
fn shortfall(resolved: usize, reported: i32) -> Option<ReferenceGap> {
    let expected = usize::try_from(reported).unwrap_or(0);
    (resolved < expected).then_some(ReferenceGap::Shortfall { resolved, expected })
}

/// The references for the IDs that resolved to a DOI, in reference order.
fn references_in_order(ids: &[String], dois: &HashMap<String, String>) -> Vec<Reference> {
    ids.iter()
        .filter_map(|id| dois.get(id))
        .map(|doi| doi_reference(doi.clone()))
        .collect()
}

/// A reference known only by its DOI.
fn doi_reference(doi: String) -> Reference {
    Reference {
        doi: Some(doi),
        key: None,
        raw_unstructured: None,
        article_title: None,
        author: None,
        year: None,
        volume: None,
        first_page: None,
    }
}

/// reqwest puts the request URL in its error text, and the URL carries the key.
fn transport(error: reqwest::Error) -> OpenAlexError {
    OpenAlexError::Request(error.without_url())
}

/// Maps a 200 body after the identity check. A sibling record filed under another
/// DOI, or a null DOI, is not the work that was asked for, so it is not found. Front
/// matter is rejected here, before any reference list is resolved.
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
    if body.is_paratext {
        return Err(OpenAlexError::Paratext);
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
            self.fetch_metered(doi, permit)
                .await
                .map_err(OpenAlexError::into_fetch_error)
        })
    }
}

/// The import-path record for an OpenAlex work, mirroring
/// `raw_record_from_crossref_work`. Authors come from `authorships` as literal names.
/// An authorship without a display name is skipped.
#[must_use]
pub fn raw_record_from_openalex_work(work: WorkWithReferences) -> RawRecord {
    let doi = work.work.doi.clone();
    let authors = work
        .raw
        .get("authorships")
        .and_then(Value::as_array)
        .map(|authorships| {
            authorships
                .iter()
                .filter_map(|authorship| {
                    authorship
                        .pointer("/author/display_name")
                        .and_then(Value::as_str)
                })
                .map(RawAuthor::literal)
                .collect()
        })
        .unwrap_or_default();
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

#[derive(Debug, Error)]
pub enum OpenAlexError {
    #[error("a contact e-mail address is required; the server must set DEEPREF_CONTACT_EMAIL")]
    MissingContactEmail,
    #[error(transparent)]
    Permit(#[from] PermitError),
    #[error("OpenAlex returned not found for {0}")]
    NotFound(String),
    #[error("OpenAlex lists the work as paratext")]
    Paratext,
    #[error("OpenAlex answered {returned:?} for a request for {requested}")]
    DoiMismatch {
        requested: String,
        returned: Option<String>,
    },
    #[error("OpenAlex refused a keyless request: {0}")]
    Unauthorized(StatusCode),
    #[error("OpenAlex returned retryable status: {0}")]
    RetryableStatus(StatusCode),
    #[error("OpenAlex's daily budget is spent; list calls stop until the reset")]
    BudgetExhausted { reset_secs: Option<u64> },
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
            // The reason the worker stores as the item's last error. Crossref returns the same
            // front matter as a normal record, so no later provider is asked.
            Self::Paratext => ProviderFetchError::Rejected {
                provider,
                reason: "paratext".to_owned(),
            },
            Self::Unauthorized(_)
            | Self::RetryableStatus(_)
            | Self::BudgetExhausted { .. }
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
    use serde_json::json;
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
    /// The retraction notice for Wakefield's 1998 paper, as OpenAlex answered it.
    const RETRACTED_DOI: &str = "10.1016/s0140-6736(97)11096-0";
    const RETRACTED: &str = include_str!("../tests/fixtures/openalex/work_retracted.json");
    /// A real list-call response for the first ten references of `WORK`.
    const REFERENCES_LIST: &str = include_str!("../tests/fixtures/openalex/references_list.json");
    const EMPTY_LIST: &str = r#"{"meta":{"count":0},"results":[]}"#;

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
        client_in(OpenAlexCorpus::Core, base_url, api_key, max_attempts)
    }

    fn client_in(
        corpus: OpenAlexCorpus,
        base_url: String,
        api_key: Option<&str>,
        max_attempts: usize,
    ) -> OpenAlexClient {
        OpenAlexClient::with_base_url(
            base_url,
            "ops@example.org",
            api_key.map(str::to_owned),
            corpus,
        )
        .expect("client should build")
        .with_max_attempts(max_attempts)
    }

    /// A list response carrying `work` as its first and only result.
    fn corpus_hit(work: &str) -> String {
        let work: Value = serde_json::from_str(work).expect("fixture is JSON");
        json!({ "results": [work] }).to_string()
    }

    async fn fetch(
        client: &OpenAlexClient,
        doi: &str,
        permit: &RecordingPermit,
    ) -> Result<FetchedWork, ProviderFetchError> {
        WorkProvider::fetch_work_with_references(client, doi, permit).await
    }

    /// `WORK` with its `referenced_works` replaced by `ids`.
    fn work_citing(ids: &[String]) -> String {
        let mut body: Value = serde_json::from_str(WORK).expect("fixture is JSON");
        body["referenced_works"] = json!(
            ids.iter()
                .map(|id| format!("https://openalex.org/{id}"))
                .collect::<Vec<_>>()
        );
        body["referenced_works_count"] = json!(ids.len());
        body.to_string()
    }

    fn synthetic_ids(count: usize) -> Vec<String> {
        (0..count)
            .map(|index| format!("W{}", 1_000_000 + index))
            .collect()
    }

    /// A list response resolving each of `ids` to `10.9999/<lowercase id>`, except the
    /// IDs in `without_doi`, which come back with a null DOI.
    fn list_body(ids: &[String], without_doi: &[&str]) -> String {
        let results: Vec<Value> = ids
            .iter()
            .map(|id| {
                let doi = (!without_doi.contains(&id.as_str()))
                    .then(|| format!("https://doi.org/10.9999/{}", id.to_lowercase()));
                json!({"id": format!("https://openalex.org/{id}"), "doi": doi})
            })
            .collect();
        json!({"meta": {"count": results.len()}, "results": results, "group_by": []}).to_string()
    }

    /// The IDs in a list request's `ids.openalex:` filter.
    fn filter_ids(request_line: &str) -> Vec<&str> {
        request_line
            .split("filter=ids.openalex:")
            .nth(1)
            .unwrap_or_default()
            .split('&')
            .next()
            .unwrap_or_default()
            .split('|')
            .collect()
    }

    fn dois(fetched: &FetchedWork) -> Vec<&str> {
        fetched
            .work
            .references
            .iter()
            .filter_map(|reference| reference.doi.as_deref())
            .collect()
    }

    #[tokio::test]
    async fn a_found_work_is_answered_on_the_callers_permit_with_the_selected_fields() {
        let (base, requests) =
            scripted_server(vec![reply("200 OK", WORK), reply("200 OK", EMPTY_LIST)]).await;
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
        assert_eq!(
            permit.taken(),
            vec![ProviderKind::OpenAlex],
            "the singleton runs on the caller's permit; the one list call takes its own"
        );
        let requests = requests.lock().expect("request log");
        assert_eq!(requests.len(), 2);
        assert!(
            requests[1].starts_with("GET /works?filter=ids.openalex:"),
            "{}",
            requests[1]
        );
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
        let select = requests[0]
            .split("select=")
            .nth(1)
            .and_then(|rest| rest.split(['&', ' ']).next())
            .unwrap_or_default();
        let fields: Vec<&str> = select.split(',').collect();
        for field in [
            "counts_by_year",
            "awards",
            "funders",
            "topics",
            "authorships",
            "is_retracted",
            "is_paratext",
            "best_oa_location",
            "locations",
        ] {
            assert!(fields.contains(&field), "{field} is not selected: {select}");
        }
    }

    #[tokio::test]
    async fn the_mapping_carries_the_fixture_fields_and_rebuilds_the_abstract() {
        let (base, _) =
            scripted_server(vec![reply("200 OK", WORK), reply("200 OK", EMPTY_LIST)]).await;
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
        assert_eq!(
            work.raw["deepref_oa_urls"]["best_oa_location"],
            json!({
                "url": "https://doi.org/10.1371/journal.pmed.0020124",
                "pdf_url": null,
                "license": "cc-by",
                "version": "publishedVersion",
                "is_oa": true,
                "source": "PLoS Medicine",
            })
        );
        assert_eq!(
            work.raw["deepref_oa_urls"]["locations"][1],
            json!({
                "url": "https://pubmed.ncbi.nlm.nih.gov/16060722",
                "pdf_url": null,
                "license": null,
                "version": "publishedVersion",
                "is_oa": false,
                "source": "PubMed",
            })
        );
        assert_eq!(
            work.raw["authorships"][0]["institutions"][0]["display_name"],
            "University of Ioannina"
        );
        assert_eq!(
            work.raw["counts_by_year"][0],
            json!({ "year": 2026, "cited_by_count": 594 })
        );
        assert_eq!(work.raw["awards"], json!([]));
        assert_eq!(work.raw["funders"], json!([]));
    }

    #[tokio::test]
    async fn a_retracted_work_keeps_its_funders_counts_and_topics_in_raw() {
        let (base, _) = scripted_server(vec![
            reply("200 OK", RETRACTED),
            reply("200 OK", EMPTY_LIST),
        ])
        .await;

        let fetched = fetch(
            &client(base, None, 1),
            RETRACTED_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect("a retracted work is still answered");

        assert_eq!(
            fetched.work.raw["funders"][0]["display_name"],
            "Royal Free London NHS Foundation Trust"
        );
        assert_eq!(
            fetched.work.raw["counts_by_year"][0]["year"],
            Value::from(2026)
        );
        assert_eq!(fetched.work.raw["topics"].as_array().map(Vec::len), Some(3));
    }

    #[tokio::test]
    async fn referenced_works_resolve_to_dois_in_reference_order_through_one_list_call() {
        let (base, requests) = scripted_server(vec![
            reply("200 OK", WORK),
            reply("200 OK", REFERENCES_LIST),
        ])
        .await;
        let permit = RecordingPermit::default();

        let fetched = fetch(&client(base, None, 1), ABSTRACT_DOI, &permit)
            .await
            .expect("openalex should answer");

        assert_eq!(
            dois(&fetched),
            vec![
                "10.1186/1471-2288-1-2",
                "10.1038/35015718",
                "10.1038/nrc1322",
                "10.1038/ng749",
                "10.1186/1471-2288-1-3",
                "10.1056/nejmp048286",
                "10.1001/jama.283.15.2008",
                "10.1002/sim.4780030421",
            ],
            "the two targets without a DOI are dropped and the rest keep reference order"
        );
        assert_eq!(fetched.work.work.references_count, 40);
        assert_eq!(permit.taken(), vec![ProviderKind::OpenAlex]);
        let requests = requests.lock().expect("request log");
        assert_eq!(requests.len(), 2);
        assert_eq!(filter_ids(&requests[1]).len(), 40);
        assert!(
            requests[1].starts_with("GET /works?filter=ids.openalex:W1498364240|W1501619595|"),
            "{}",
            requests[1]
        );
        assert!(
            requests[1].contains("&select=id,doi&per_page=100&mailto="),
            "{}",
            requests[1]
        );
    }

    #[tokio::test]
    async fn thirty_references_resolve_in_one_list_call() {
        let ids = synthetic_ids(30);
        let (base, requests) = scripted_server(vec![
            reply("200 OK", work_citing(&ids)),
            reply("200 OK", list_body(&ids, &[])),
        ])
        .await;

        let fetched = fetch(
            &client(base, None, 1),
            ABSTRACT_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect("openalex should answer");

        assert_eq!(fetched.work.references.len(), 30);
        assert_eq!(dois(&fetched)[0], "10.9999/w1000000");
        let requests = requests.lock().expect("request log");
        assert_eq!(requests.len(), 2, "one singleton and one list call");
        assert_eq!(
            filter_ids(&requests[1]),
            ids.iter().map(String::as_str).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn two_hundred_fifty_references_take_three_list_calls_of_100_100_50() {
        let ids = synthetic_ids(250);
        let (base, requests) = scripted_server(vec![
            reply("200 OK", work_citing(&ids)),
            reply("200 OK", list_body(&ids[..100], &[])),
            reply("200 OK", list_body(&ids[100..200], &[])),
            reply("200 OK", list_body(&ids[200..], &[])),
        ])
        .await;
        let permit = RecordingPermit::default();

        let fetched = fetch(&client(base, None, 1), ABSTRACT_DOI, &permit)
            .await
            .expect("openalex should answer");

        assert_eq!(fetched.work.references.len(), 250);
        assert_eq!(
            permit.taken(),
            vec![ProviderKind::OpenAlex; 3],
            "each list call takes one permit"
        );
        let requests = requests.lock().expect("request log");
        assert_eq!(requests.len(), 4, "one singleton and three list calls");
        let chunks: Vec<usize> = requests[1..]
            .iter()
            .map(|request| filter_ids(request).len())
            .collect();
        assert_eq!(chunks, vec![100, 100, 50]);
    }

    #[tokio::test]
    async fn list_dois_are_normalized_and_a_target_missing_from_the_response_is_dropped() {
        let ids = vec!["W1".to_owned(), "W2".to_owned(), "W3".to_owned()];
        let (base, _) = scripted_server(vec![
            reply("200 OK", work_citing(&ids)),
            reply(
                "200 OK",
                r#"{"results":[
                    {"id":"https://openalex.org/W3","doi":"https://doi.org/10.1000/UPPER.Case"},
                    {"id":"https://openalex.org/W1","doi":null}
                ]}"#,
            ),
        ])
        .await;

        let fetched = fetch(
            &client(base, None, 1),
            ABSTRACT_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect("openalex should answer");

        assert_eq!(dois(&fetched), vec!["10.1000/upper.case"]);
    }

    #[tokio::test]
    async fn malformed_reference_ids_stay_out_of_the_list_filter() {
        let mut body: Value = serde_json::from_str(&work_citing(&["W1".to_owned()]))
            .expect("the citing work is JSON");
        body["referenced_works"] = json!([
            "https://openalex.org/W1",
            "https://openalex.org/W2|W3",
            "https://example.org/W4",
        ]);
        let (base, requests) = scripted_server(vec![
            reply("200 OK", body.to_string()),
            reply("200 OK", EMPTY_LIST),
        ])
        .await;

        fetch(
            &client(base, None, 1),
            ABSTRACT_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect("openalex should answer");

        let requests = requests.lock().expect("request log");
        assert_eq!(filter_ids(&requests[1]), vec!["W1"]);
    }

    #[tokio::test]
    async fn a_list_rate_limit_with_budget_left_is_retried_then_answers() {
        let (base, requests) = scripted_server(vec![
            reply("200 OK", WORK),
            Reply {
                status: "429 Too Many Requests",
                headers: "X-RateLimit-Remaining: 5\r\n".to_owned(),
                body: "{}".to_owned(),
            },
            reply("200 OK", REFERENCES_LIST),
        ])
        .await;
        let permit = RecordingPermit::default();

        let fetched = fetch(&client(base, None, 2), ABSTRACT_DOI, &permit)
            .await
            .expect("the retried list call should answer");

        assert_eq!(dois(&fetched).len(), 8);
        assert_eq!(
            permit.taken(),
            vec![ProviderKind::OpenAlex; 2],
            "the retry takes a permit of its own"
        );
        assert_eq!(requests.lock().expect("request log").len(), 3);
    }

    #[tokio::test]
    async fn a_list_rate_limit_with_budget_left_is_retryable_once_attempts_run_out() {
        let limited = || Reply {
            status: "429 Too Many Requests",
            headers: "X-RateLimit-Remaining: 5\r\n".to_owned(),
            body: "{}".to_owned(),
        };
        let (base, _) = scripted_server(vec![reply("200 OK", WORK), limited(), limited()]).await;

        let error = fetch(
            &client(base, None, 2),
            ABSTRACT_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect_err("the list call should not answer");

        match error {
            ProviderFetchError::Retryable { provider, message } => {
                assert_eq!(provider, ProviderKind::OpenAlex);
                assert!(message.contains("429"), "{message}");
            }
            other => panic!("expected a retryable failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_budget_exhausted_list_call_keeps_the_metadata_and_stops_later_list_calls() {
        let (base, requests) = scripted_server(vec![
            reply("200 OK", WORK),
            Reply {
                status: "429 Too Many Requests",
                headers: "X-RateLimit-Limit: 1000\r\nX-RateLimit-Remaining: 0\r\nX-RateLimit-Reset: 3600\r\n"
                    .to_owned(),
                body: "{}".to_owned(),
            },
            reply("200 OK", WORK),
            reply("200 OK", NO_REFERENCES),
        ])
        .await;
        let client = client(base, None, 1);
        let expected_gap = Some(ReferenceGap::BudgetExhausted {
            resolved: 0,
            expected: 40,
            reset_secs: Some(3600),
        });

        let first = fetch(&client, ABSTRACT_DOI, &RecordingPermit::default())
            .await
            .expect("the singleton still answers when the budget is spent");
        assert_eq!(
            first.work.work.title.as_deref(),
            Some("Why Most Published Research Findings Are False")
        );
        assert!(first.work.references.is_empty());
        assert_eq!(first.reference_gap, expected_gap);

        let later = fetch(&client, ABSTRACT_DOI, &RecordingPermit::default())
            .await
            .expect("a later work answers from its singleton");
        assert!(later.work.references.is_empty());
        assert_eq!(later.reference_gap, expected_gap);

        let unreferenced = fetch(&client, NO_REFERENCES_DOI, &RecordingPermit::default())
            .await
            .expect("a work without references answers");
        assert_eq!(
            unreferenced.reference_gap, None,
            "nothing was left to look up"
        );

        let requests = requests.lock().expect("request log");
        assert_eq!(requests.len(), 4);
        assert!(
            requests[1].starts_with("GET /works?filter=ids.openalex:"),
            "{}",
            requests[1]
        );
        for request in [&requests[2], &requests[3]] {
            assert!(request.starts_with("GET /works/doi:"), "{request}");
        }
    }

    #[tokio::test]
    async fn a_partial_budget_stop_keeps_the_references_resolved_so_far() {
        let ids = synthetic_ids(250);
        let (base, requests) = scripted_server(vec![
            reply("200 OK", work_citing(&ids)),
            reply("200 OK", list_body(&ids[..100], &[])),
            Reply {
                status: "429 Too Many Requests",
                headers: "X-RateLimit-Remaining: 0\r\nX-RateLimit-Reset: 120\r\n".to_owned(),
                body: "{}".to_owned(),
            },
        ])
        .await;

        let fetched = fetch(
            &client(base, None, 1),
            ABSTRACT_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect("the metadata still answers");

        assert_eq!(fetched.work.references.len(), 100);
        assert_eq!(dois(&fetched)[99], "10.9999/w1000099");
        assert_eq!(
            fetched.reference_gap,
            Some(ReferenceGap::BudgetExhausted {
                resolved: 100,
                expected: 250,
                reset_secs: Some(120),
            })
        );
        assert_eq!(requests.lock().expect("request log").len(), 3);
    }

    #[tokio::test]
    async fn a_list_that_resolves_fewer_dois_than_the_work_reports_is_a_shortfall() {
        let ids = synthetic_ids(3);
        let (base, _) = scripted_server(vec![
            reply("200 OK", work_citing(&ids)),
            reply("200 OK", list_body(&ids, &[ids[1].as_str()])),
        ])
        .await;

        let fetched = fetch(
            &client(base, None, 1),
            ABSTRACT_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect("the metadata answers");

        assert_eq!(dois(&fetched), vec!["10.9999/w1000000", "10.9999/w1000002"]);
        assert_eq!(
            fetched.reference_gap,
            Some(ReferenceGap::Shortfall {
                resolved: 2,
                expected: 3,
            })
        );
    }

    #[tokio::test]
    async fn an_empty_reference_list_under_a_reported_count_is_a_shortfall() {
        let (base, _) =
            scripted_server(vec![reply("200 OK", WORK), reply("200 OK", EMPTY_LIST)]).await;

        let fetched = fetch(
            &client(base, None, 1),
            ABSTRACT_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect("the metadata answers");

        assert_eq!(
            fetched.reference_gap,
            Some(ReferenceGap::Shortfall {
                resolved: 0,
                expected: 40,
            })
        );
    }

    #[tokio::test]
    async fn a_complete_reference_list_has_no_gap() {
        let ids = synthetic_ids(3);
        let (base, _) = scripted_server(vec![
            reply("200 OK", work_citing(&ids)),
            reply("200 OK", list_body(&ids, &[])),
        ])
        .await;

        let fetched = fetch(
            &client(base, None, 1),
            ABSTRACT_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect("the metadata answers");

        assert_eq!(fetched.reference_gap, None);
    }

    #[tokio::test]
    async fn a_budget_stop_is_not_retried() {
        let (base, _) = scripted_server(vec![
            reply("200 OK", WORK),
            Reply {
                status: "429 Too Many Requests",
                headers: "X-RateLimit-Remaining: 0\r\nX-RateLimit-Reset: 60\r\n".to_owned(),
                body: "{}".to_owned(),
            },
        ])
        .await;
        let permit = RecordingPermit::default();

        let fetched = fetch(&client(base, None, 5), ABSTRACT_DOI, &permit)
            .await
            .expect("the metadata still answers");

        assert!(fetched.reference_gap.is_some());
        assert_eq!(
            permit.taken(),
            vec![ProviderKind::OpenAlex],
            "one permit for the one list call that ran"
        );
    }

    #[tokio::test]
    async fn a_low_budget_response_claims_the_daily_warning_and_a_quiet_one_does_not() {
        let quiet = scripted_server(vec![reply("200 OK", WORK), reply("200 OK", EMPTY_LIST)]).await;
        let low = scripted_server(vec![
            Reply {
                status: "200 OK",
                headers: "X-RateLimit-Limit: 1000\r\nX-RateLimit-Remaining: 50\r\n".to_owned(),
                body: WORK.to_owned(),
            },
            reply("200 OK", EMPTY_LIST),
        ])
        .await;
        let quiet_client = client(quiet.0, None, 1);
        let low_client = client(low.0, None, 1);

        fetch(&quiet_client, ABSTRACT_DOI, &RecordingPermit::default())
            .await
            .expect("openalex should answer");
        fetch(&low_client, ABSTRACT_DOI, &RecordingPermit::default())
            .await
            .expect("openalex should answer");

        assert_eq!(*lock(&quiet_client.budget.warned_on), None);
        assert_eq!(
            *lock(&low_client.budget.warned_on),
            Some(Utc::now().date_naive())
        );
    }

    #[test]
    fn a_budget_stop_holds_until_the_reset_and_then_expires() {
        let budget = ListBudget::default();
        let start = Instant::now();

        assert_eq!(budget.stopped(start), None);
        budget.stop(start, Some(60));
        assert_eq!(
            budget.stopped(start + Duration::from_secs(59)),
            Some(Some(60))
        );
        assert_eq!(
            budget.stopped(start + Duration::from_secs(60)),
            None,
            "list calls resume once the reset has passed"
        );
    }

    #[test]
    fn a_budget_stop_without_a_reset_header_lasts_to_the_next_utc_midnight() {
        let budget = ListBudget::default();
        let start = Instant::now();

        budget.stop(start, None);

        assert_eq!(budget.stopped(start), Some(None));
        assert_eq!(budget.stopped(start + Duration::from_secs(86_400)), None);
    }

    #[test]
    fn the_low_budget_warning_is_under_a_tenth_and_claimed_once_per_utc_day() {
        assert!(!budget_is_low(1000, 999));
        assert!(
            !budget_is_low(1000, 100),
            "a tenth exactly is not under a tenth"
        );
        assert!(budget_is_low(1000, 99));

        let budget = ListBudget::default();
        let day = NaiveDate::from_ymd_opt(2026, 10, 18).expect("valid date");
        assert!(budget.claim_daily_warning(day));
        assert!(!budget.claim_daily_warning(day));
        assert!(budget.claim_daily_warning(day.succ_opt().expect("next day")));
    }

    #[tokio::test]
    async fn corpus_all_answers_a_work_the_core_corpus_misses() {
        let (base, requests) = scripted_server(vec![
            reply("404 Not Found", NOT_FOUND),
            reply("200 OK", corpus_hit(NO_REFERENCES)),
        ])
        .await;
        let permit = RecordingPermit::default();

        let fetched = fetch(
            &client_in(OpenAlexCorpus::All, base, None, 1),
            NO_REFERENCES_DOI,
            &permit,
        )
        .await
        .expect("the corpus=all retry should answer");

        assert_eq!(fetched.work.work.doi, NO_REFERENCES_DOI);
        assert_eq!(fetched.provider, ProviderKind::OpenAlex);
        assert_eq!(permit.taken(), vec![ProviderKind::OpenAlex]);
        let requests = requests.lock().expect("request log");
        assert_eq!(requests.len(), 2);
        assert!(
            requests[1].starts_with(
                "GET /works?filter=doi:10.5281/zenodo.1&corpus=all&select=id,doi,display_name,"
            ),
            "{}",
            requests[1]
        );
    }

    #[tokio::test]
    async fn a_corpus_all_miss_is_not_found() {
        let (base, _) = scripted_server(vec![
            reply("404 Not Found", NOT_FOUND),
            reply("200 OK", EMPTY_LIST),
        ])
        .await;

        let error = fetch(
            &client_in(OpenAlexCorpus::All, base, None, 1),
            NO_REFERENCES_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect_err("nothing in the corpus should answer");

        assert_eq!(
            error,
            ProviderFetchError::NotFound {
                provider: ProviderKind::OpenAlex,
                doi: NO_REFERENCES_DOI.to_owned(),
            }
        );
    }

    #[tokio::test]
    async fn the_first_corpus_all_hit_must_carry_the_requested_doi() {
        let (base, _) = scripted_server(vec![
            reply("404 Not Found", NOT_FOUND),
            reply("200 OK", corpus_hit(WORK)),
        ])
        .await;

        let error = fetch(
            &client_in(OpenAlexCorpus::All, base, None, 1),
            NO_REFERENCES_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect_err("a record filed under another DOI must not answer");

        assert!(matches!(
            error,
            ProviderFetchError::NotFound {
                provider: ProviderKind::OpenAlex,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn corpus_all_is_not_asked_while_the_budget_is_stopped() {
        let (base, requests) = scripted_server(vec![
            reply("200 OK", WORK),
            Reply {
                status: "429 Too Many Requests",
                headers: "X-RateLimit-Remaining: 0\r\nX-RateLimit-Reset: 600\r\n".to_owned(),
                body: "{}".to_owned(),
            },
            reply("404 Not Found", NOT_FOUND),
        ])
        .await;
        let client = client_in(OpenAlexCorpus::All, base, None, 1);
        fetch(&client, ABSTRACT_DOI, &RecordingPermit::default())
            .await
            .expect("the singleton still answers");

        let error = fetch(&client, NO_REFERENCES_DOI, &RecordingPermit::default())
            .await
            .expect_err("a stopped budget answers not found");

        assert!(matches!(
            error,
            ProviderFetchError::NotFound {
                provider: ProviderKind::OpenAlex,
                ..
            }
        ));
        let requests = requests.lock().expect("request log");
        assert_eq!(
            requests.len(),
            3,
            "no corpus list call while the budget is stopped"
        );
        assert!(
            requests[2].starts_with("GET /works/doi:"),
            "{}",
            requests[2]
        );
    }

    #[tokio::test]
    async fn a_corpus_all_retry_that_hits_the_budget_is_not_found() {
        let (base, requests) = scripted_server(vec![
            reply("404 Not Found", NOT_FOUND),
            Reply {
                status: "429 Too Many Requests",
                headers: "X-RateLimit-Remaining: 0\r\nX-RateLimit-Reset: 60\r\n".to_owned(),
                body: "{}".to_owned(),
            },
        ])
        .await;

        let error = fetch(
            &client_in(OpenAlexCorpus::All, base, None, 5),
            NO_REFERENCES_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect_err("a retry stopped by the budget answers not found");

        assert!(matches!(
            error,
            ProviderFetchError::NotFound {
                provider: ProviderKind::OpenAlex,
                ..
            }
        ));
        assert_eq!(requests.lock().expect("request log").len(), 2);
    }

    #[tokio::test]
    async fn the_core_corpus_does_not_retry_a_missing_work() {
        let (base, requests) = scripted_server(vec![reply("404 Not Found", NOT_FOUND)]).await;

        let error = fetch(
            &client(base, None, 1),
            ABSTRACT_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect_err("a 404 in the core corpus is not found");

        assert!(matches!(error, ProviderFetchError::NotFound { .. }));
        assert_eq!(requests.lock().expect("request log").len(), 1);
    }

    #[tokio::test]
    async fn a_list_server_error_fails_the_whole_fetch_as_retryable() {
        let (base, _) =
            scripted_server(vec![reply("200 OK", WORK), reply("502 Bad Gateway", "{}")]).await;

        let error = fetch(
            &client(base, None, 1),
            ABSTRACT_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect_err("a failed list call should not answer");

        assert!(matches!(
            error,
            ProviderFetchError::Retryable {
                provider: ProviderKind::OpenAlex,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn a_rejected_key_on_a_list_call_downgrades_and_repeats_keyless() {
        let (base, requests) = scripted_server(vec![
            reply("200 OK", WORK),
            reply("401 Unauthorized", INVALID_KEY),
            reply("200 OK", REFERENCES_LIST),
        ])
        .await;
        let client = client(base, Some("test-key"), 1);
        let permit = RecordingPermit::default();

        let fetched = fetch(&client, ABSTRACT_DOI, &permit)
            .await
            .expect("the keyless repeat should resolve the references");

        assert_eq!(dois(&fetched).len(), 8);
        assert_eq!(client.key_state(), OpenAlexKeyState::Keyless);
        assert_eq!(permit.taken(), vec![ProviderKind::OpenAlex; 2]);
        let requests = requests.lock().expect("request log");
        assert!(requests[0].contains("api_key=test-key"), "{}", requests[0]);
        assert!(requests[1].contains("api_key=test-key"), "{}", requests[1]);
        assert!(!requests[2].contains("api_key"), "{}", requests[2]);
    }

    #[tokio::test]
    async fn a_work_without_references_keeps_an_empty_list_and_missing_source_fields_stay_none() {
        let (base, requests) = scripted_server(vec![reply("200 OK", NO_REFERENCES)]).await;
        let fetched = fetch(
            &client(base, None, 1),
            NO_REFERENCES_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect("openalex should answer");

        assert_eq!(fetched.work.work.references_count, 0);
        assert!(fetched.work.references.is_empty());
        assert_eq!(
            requests.lock().expect("request log").len(),
            1,
            "a work without references makes no list call"
        );
        assert_eq!(fetched.work.work.publisher, None);
        assert_eq!(
            fetched.work.work.container_title.as_deref(),
            Some("Journal of Child Development, Exceptionality and Education")
        );
        assert_eq!(fetched.work.work.published_year, Some(2025));
    }

    #[tokio::test]
    async fn a_paratext_work_is_rejected_before_any_reference_is_resolved() {
        let mut body: Value = serde_json::from_str(WORK).expect("fixture is JSON");
        body["is_paratext"] = Value::Bool(true);
        let (base, requests) = scripted_server(vec![reply("200 OK", body.to_string())]).await;

        let error = fetch(
            &client(base, None, 1),
            ABSTRACT_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect_err("front matter ends the lookup");

        assert_eq!(
            error,
            ProviderFetchError::Rejected {
                provider: ProviderKind::OpenAlex,
                reason: "paratext".to_owned(),
            }
        );
        assert_eq!(
            requests.lock().expect("request log").len(),
            1,
            "no list call runs for front matter"
        );
    }

    #[tokio::test]
    async fn a_retracted_work_is_answered_and_keeps_its_flag_in_raw() {
        let (base, _) = scripted_server(vec![
            reply("200 OK", RETRACTED),
            reply("200 OK", EMPTY_LIST),
        ])
        .await;

        let fetched = fetch(
            &client(base, None, 1),
            RETRACTED_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect("a retracted work is still answered");

        assert_eq!(fetched.work.work.doi, RETRACTED_DOI);
        assert!(
            fetched
                .work
                .work
                .title
                .as_deref()
                .is_some_and(|title| title.starts_with("RETRACTED:")),
            "{:?}",
            fetched.work.work.title
        );
        assert_eq!(fetched.work.raw["is_retracted"], Value::Bool(true));
    }

    #[tokio::test]
    async fn a_redirect_is_followed_and_the_matching_work_answers() {
        let (base, requests) = scripted_server(vec![
            redirect("/works/W2144981148"),
            reply("200 OK", WORK),
            reply("200 OK", EMPTY_LIST),
        ])
        .await;

        let fetched = fetch(
            &client(base, None, 1),
            ABSTRACT_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect("the redirected work should answer");

        assert_eq!(fetched.work.work.doi, ABSTRACT_DOI);
        assert_eq!(requests.lock().expect("request log").len(), 3);
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
            reply("200 OK", EMPTY_LIST),
        ])
        .await;
        let permit = RecordingPermit::default();

        let fetched = fetch(&client(base, None, 2), ABSTRACT_DOI, &permit)
            .await
            .expect("the retry should answer");

        assert_eq!(fetched.provider, ProviderKind::OpenAlex);
        assert_eq!(
            permit.taken(),
            vec![ProviderKind::OpenAlex; 2],
            "the singleton retry and the list call each take a permit"
        );
        assert_eq!(requests.lock().expect("request log").len(), 3);
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
            reply("200 OK", EMPTY_LIST),
            reply("200 OK", WORK),
            reply("200 OK", EMPTY_LIST),
        ])
        .await;
        let client = client(base, Some("test-key"), 1);
        let permit = RecordingPermit::default();

        let fetched = fetch(&client, ABSTRACT_DOI, &permit)
            .await
            .expect("the keyless repeat should answer");

        assert_eq!(fetched.provider, ProviderKind::OpenAlex);
        assert_eq!(
            permit.taken(),
            vec![ProviderKind::OpenAlex; 2],
            "the keyless repeat and the list call each take a permit"
        );
        assert_eq!(client.key_state(), OpenAlexKeyState::Keyless);
        assert_eq!(client.rate_per_second(), KEYLESS_RATE_PER_SECOND);

        let second = RecordingPermit::default();
        fetch(&client, ABSTRACT_DOI, &second)
            .await
            .expect("later lookups stay keyless and answer");
        assert_eq!(
            second.taken(),
            vec![ProviderKind::OpenAlex],
            "only the list call takes a permit"
        );

        let requests = requests.lock().expect("request log");
        assert_eq!(requests.len(), 5);
        assert!(requests[0].contains("api_key=test-key"), "{}", requests[0]);
        for request in &requests[1..] {
            assert!(!request.contains("api_key"), "{request}");
        }
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

    #[tokio::test]
    async fn the_raw_record_carries_the_work_as_an_import_would() {
        let (base, _) = scripted_server(vec![reply("200 OK", NO_REFERENCES)]).await;
        let fetched = fetch(
            &client(base, None, 1),
            NO_REFERENCES_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect("openalex should answer");

        let record = raw_record_from_openalex_work(fetched.work);

        assert_eq!(record.source_identifiers.len(), 1);
        assert_eq!(record.source_identifiers[0].scheme, IdentifierScheme::Doi);
        assert_eq!(
            record.source_identifiers[0].normalized_value,
            NO_REFERENCES_DOI
        );
        assert_eq!(
            record.authors,
            vec![
                RawAuthor::literal("Eduard C. Groen"),
                RawAuthor::literal("Fabiano Dalpiaz"),
            ]
        );
        assert_eq!(record.publication_year, Some(2025));
        assert_eq!(
            record.journal.as_deref(),
            Some("Journal of Child Development, Exceptionality and Education")
        );
        assert!(record.abstract_text.is_some());
    }

    #[tokio::test]
    async fn the_raw_record_keeps_orcids_in_the_raw_payload_and_skips_unnamed_authors() {
        let (base, _) =
            scripted_server(vec![reply("200 OK", WORK), reply("200 OK", EMPTY_LIST)]).await;
        let mut fetched = fetch(
            &client(base, None, 1),
            ABSTRACT_DOI,
            &RecordingPermit::default(),
        )
        .await
        .expect("openalex should answer");
        let raw_orcid = fetched.work.raw["authorships"][0]["author"]["orcid"].clone();

        let named = raw_record_from_openalex_work(fetched.work.clone());
        assert_eq!(
            named.authors,
            vec![RawAuthor::literal("John P. A. Ioannidis")]
        );
        assert_eq!(
            named.title.as_deref(),
            Some("Why Most Published Research Findings Are False")
        );
        assert_eq!(named.publication_year, Some(2005));
        assert_eq!(named.raw["authorships"][0]["author"]["orcid"], raw_orcid);

        fetched.work.raw["authorships"] = serde_json::json!([
            {"author": {"display_name": null, "orcid": null}},
            {"author": {"display_name": "Ana Pereira"}}
        ]);
        let unnamed = raw_record_from_openalex_work(fetched.work);
        assert_eq!(unnamed.authors, vec![RawAuthor::literal("Ana Pereira")]);
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
        assert_eq!(
            OpenAlexError::Paratext.into_fetch_error(),
            ProviderFetchError::Rejected {
                provider,
                reason: "paratext".to_owned(),
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
            OpenAlexError::BudgetExhausted {
                reset_secs: Some(60)
            }
            .into_fetch_error(),
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
