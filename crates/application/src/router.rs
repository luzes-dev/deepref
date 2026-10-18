//! Routes one DOI through the metadata provider chain: the allowlist, the decision
//! matrix and the provenance written on success. Pure: adapters and the permit
//! callback come in as arguments, and the router makes no HTTP or database calls.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use deepref_domain::{ProviderKind, Reference, normalize_doi};
use serde::Serialize;
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::acquisition::{
    AttemptPurpose, FetchedWork, OutboundRequest, OutboundService, PermitError, ProviderFetchError,
    ProviderFuture, ProviderPermit, ReferenceReport, WorkProvider,
};

/// What one attempt produced. Serialized into `raw.deepref_provider_attempts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AttemptOutcome {
    Answered,
    /// The daily budget stopped paid calls. `reset_secs` is the reset the provider sent.
    BudgetExhausted {
        reset_secs: Option<u64>,
    },
    NotFound {
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Rejected {
        reason: String,
    },
    Retryable {
        message: String,
    },
    Fatal {
        message: String,
    },
}

impl AttemptOutcome {
    /// The snake_case name this outcome serializes to, for logs.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Answered => "answered",
            Self::BudgetExhausted { .. } => "budget_exhausted",
            Self::NotFound { .. } => "not_found",
            Self::Rejected { .. } => "rejected",
            Self::Retryable { .. } => "retryable",
            Self::Fatal { .. } => "fatal",
        }
    }

    /// The outcome an attempt records when its adapter failed with `error`.
    pub fn from_error(error: &ProviderFetchError) -> Self {
        match error {
            ProviderFetchError::NotFound { reason, .. } => Self::NotFound {
                reason: reason.clone(),
            },
            ProviderFetchError::Rejected { reason, .. } => Self::Rejected {
                reason: reason.clone(),
            },
            ProviderFetchError::Retryable { message, .. }
            | ProviderFetchError::ReferenceRetryable { message, .. } => Self::Retryable {
                message: message.clone(),
            },
            ProviderFetchError::Fatal { message, .. }
            | ProviderFetchError::ReferenceFatal { message, .. } => Self::Fatal {
                message: message.clone(),
            },
        }
    }
}

/// One group of requests sent to one service for one purpose. `requests` counts the
/// permits that were granted, and every outbound request takes one immediately before it
/// is sent, so retries and redirect hops are included. A refused permit sends nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderAttempt {
    pub provider: OutboundService,
    pub purpose: AttemptPurpose,
    pub requests: u32,
    #[serde(flatten)]
    pub outcome: AttemptOutcome,
}

/// Why no provider answered. A rejected answer is a not-found that carries its reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    NotFound { reason: Option<String> },
    Retryable { message: String },
    Fatal { message: String },
}

#[derive(Debug, Clone)]
pub struct RouteOutput {
    /// Every attempt in the order it began, on failure as well as success.
    pub attempts: Vec<ProviderAttempt>,
    pub result: Result<FetchedWork, RouteError>,
    /// The reference status of an answer, `None` when nothing answered.
    pub references: Option<ReferenceSummary>,
}

/// Why an answer's references are degraded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DegradedReason {
    BudgetExhausted,
    OmittedTargets,
    DoiLessTargets,
    /// The Crossref reference helper failed in a way retrying cannot fix.
    HelperFatal,
}

impl DegradedReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BudgetExhausted => "budget_exhausted",
            Self::OmittedTargets => "omitted_targets",
            Self::DoiLessTargets => "doi_less_targets",
            Self::HelperFatal => "helper_fatal",
        }
    }
}

/// `raw.deepref_reference_status`: whether the required reference plan completed, and what
/// it lost when it did not. `complete` does not claim the provider captured the whole
/// bibliography. The cache keeps the work with this record, so a later attachment still sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReferenceSummary {
    pub status: &'static str,
    pub reasons: Vec<DegradedReason>,
    pub targets: usize,
    pub retained: usize,
    pub omitted: usize,
    pub doi_less: usize,
    pub unattempted: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_reset_secs: Option<u64>,
}

impl ReferenceSummary {
    fn new(report: &ReferenceReport, helper_fatal: bool) -> Self {
        let mut reasons = Vec::new();
        if report.budget_stop.is_some() {
            reasons.push(DegradedReason::BudgetExhausted);
        }
        if report.omitted > 0 {
            reasons.push(DegradedReason::OmittedTargets);
        }
        if report.doi_less > 0 {
            reasons.push(DegradedReason::DoiLessTargets);
        }
        if helper_fatal {
            reasons.push(DegradedReason::HelperFatal);
        }
        Self {
            status: if reasons.is_empty() {
                "complete"
            } else {
                "degraded"
            },
            reasons,
            targets: report.targets,
            retained: report.retained,
            omitted: report.omitted,
            doi_less: report.doi_less,
            unattempted: report.unattempted,
            budget_reset_secs: report.budget_stop.and_then(|stop| stop.reset_secs),
        }
    }

    pub fn is_degraded(&self) -> bool {
        self.status == "degraded"
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RouterError {
    #[error("no metadata provider is both in the deployment chain and implemented")]
    NoEnabledProvider,
    #[error("more than one adapter is registered for {}", .0.as_str())]
    DuplicateAdapter(ProviderKind),
}

pub struct ProviderRouter {
    deployment_chain: Vec<ProviderKind>,
    adapters: HashMap<ProviderKind, Box<dyn WorkProvider>>,
}

/// Counts the permits an adapter call is granted, by service and purpose, and passes each
/// request through to the real permit. The count is the attempt's request count.
struct CountingPermit<'p> {
    inner: &'p dyn ProviderPermit,
    granted: Mutex<Vec<(OutboundRequest, u32)>>,
}

impl<'p> CountingPermit<'p> {
    fn new(inner: &'p dyn ProviderPermit) -> Self {
        Self {
            inner,
            granted: Mutex::new(Vec::new()),
        }
    }

    fn take(&self) -> Vec<(OutboundRequest, u32)> {
        std::mem::take(&mut *self.granted.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl ProviderPermit for CountingPermit<'_> {
    fn acquire<'a>(&'a self, request: OutboundRequest) -> ProviderFuture<'a, (), PermitError> {
        Box::pin(async move {
            self.inner.acquire(request).await?;
            let mut granted = self.granted.lock().unwrap_or_else(|e| e.into_inner());
            match granted.iter_mut().find(|(seen, _)| *seen == request) {
                Some((_, count)) => *count += 1,
                None => granted.push((request, 1)),
            }
            Ok(())
        })
    }
}

/// The attempt records for one adapter call. `outcome_for` gives the outcome of each purpose
/// the adapter's own service ran, and the purposes in `always` get a record even when they
/// sent no request, because a failure or a paused budget still belongs in the history.
fn call_attempts(
    counts: Vec<(OutboundRequest, u32)>,
    kind: ProviderKind,
    always: &[AttemptPurpose],
    outcome_for: impl Fn(AttemptPurpose) -> AttemptOutcome,
) -> Vec<ProviderAttempt> {
    let own = OutboundService::Provider(kind);
    let mut attempts: Vec<ProviderAttempt> = counts
        .into_iter()
        .map(|(request, requests)| ProviderAttempt {
            provider: request.service,
            purpose: request.purpose,
            requests,
            outcome: if request.service == own {
                outcome_for(request.purpose)
            } else {
                AttemptOutcome::Answered
            },
        })
        .collect();
    for purpose in always {
        if !attempts
            .iter()
            .any(|attempt| attempt.provider == own && attempt.purpose == *purpose)
        {
            attempts.push(ProviderAttempt {
                provider: own,
                purpose: *purpose,
                requests: 0,
                outcome: outcome_for(*purpose),
            });
        }
    }
    attempts.sort_by_key(|attempt| match attempt.purpose {
        AttemptPurpose::Metadata => 0,
        AttemptPurpose::ReferenceResolution => 1,
        AttemptPurpose::ReferenceUnion => 2,
        AttemptPurpose::AbstractEnrichment => 3,
    });
    attempts
}

impl ProviderRouter {
    /// Builds the router from the deployment chain and the adapters in this build.
    pub fn new(
        deployment_chain: Vec<ProviderKind>,
        adapters: Vec<Box<dyn WorkProvider>>,
    ) -> Result<Self, RouterError> {
        let mut by_kind = HashMap::new();
        for adapter in adapters {
            let kind = adapter.kind();
            if by_kind.insert(kind, adapter).is_some() {
                return Err(RouterError::DuplicateAdapter(kind));
            }
        }
        let router = Self {
            deployment_chain: unique(deployment_chain),
            adapters: by_kind,
        };
        if router.enabled().is_empty() {
            return Err(RouterError::NoEnabledProvider);
        }
        Ok(router)
    }

    /// The deployment chain filtered to providers with an adapter. This is the allowlist.
    pub fn enabled(&self) -> Vec<ProviderKind> {
        self.deployment_chain
            .iter()
            .copied()
            .filter(|kind| self.adapters.contains_key(kind))
            .collect()
    }

    /// Runs `doi` through the chain effective for `stored`. Each adapter takes a permit
    /// immediately before every outbound request, and the router counts the permits it was
    /// granted. A permit that cannot be taken is a retryable failure of that adapter.
    ///
    /// NotFound moves on. Rejected ends the chain as not found with its reason. Retryable
    /// and Fatal are recorded and the chain moves on, so one broken provider cannot fail
    /// every DOI. When nothing answers, a retryable failure anywhere makes the fetch
    /// retryable, otherwise a fatal one makes it fatal, otherwise it is not found. Once a
    /// provider has answered, a reference failure ends the fetch without asking another.
    pub async fn fetch_work(
        &self,
        stored: &[ProviderKind],
        doi: &str,
        permit: &dyn ProviderPermit,
    ) -> RouteOutput {
        let chain: Vec<(ProviderKind, &dyn WorkProvider)> =
            effective_chain(stored, &self.enabled())
                .into_iter()
                .filter_map(|kind| {
                    self.adapters
                        .get(&kind)
                        .map(|adapter| (kind, adapter.as_ref()))
                })
                .collect();
        let mut attempts = Vec::with_capacity(chain.len());
        // Providers whose adapter ran.
        let mut tried = Vec::with_capacity(chain.len());
        let mut last_retryable = None;
        let mut last_fatal = None;
        for (kind, adapter) in chain {
            let counting = CountingPermit::new(permit);
            let result = adapter.fetch_work_after(doi, &tried, &counting).await;
            tried.push(kind);
            let counts = counting.take();
            match result {
                Ok(mut fetched) => {
                    attempts.extend(call_attempts(
                        counts,
                        kind,
                        &[AttemptPurpose::Metadata],
                        |purpose| match (purpose, fetched.references.budget_stop) {
                            (AttemptPurpose::ReferenceResolution, Some(stop)) => {
                                AttemptOutcome::BudgetExhausted {
                                    reset_secs: stop.reset_secs,
                                }
                            }
                            _ => AttemptOutcome::Answered,
                        },
                    ));
                    if let Some(stop) = fetched.references.budget_stop
                        && !attempts.iter().any(|attempt| {
                            attempt.provider == kind
                                && attempt.purpose == AttemptPurpose::ReferenceResolution
                        })
                    {
                        // A paused budget sends no request, and the history still says why.
                        attempts.push(ProviderAttempt {
                            provider: OutboundService::Provider(kind),
                            purpose: AttemptPurpose::ReferenceResolution,
                            requests: 0,
                            outcome: AttemptOutcome::BudgetExhausted {
                                reset_secs: stop.reset_secs,
                            },
                        });
                    }
                    let mut contributions = Vec::new();
                    if !fetched.work.references.is_empty() {
                        contributions.push(kind);
                    }
                    let mut helper_fatal = false;
                    if kind != ProviderKind::Crossref
                        && adapter.unions_crossref_references()
                        && let Some(crossref) = self
                            .enabled()
                            .contains(&ProviderKind::Crossref)
                            .then(|| self.adapters.get(&ProviderKind::Crossref))
                            .flatten()
                    {
                        let counting = CountingPermit::new(permit);
                        let listed = crossref.reference_list(doi, &counting).await;
                        let outcome = match &listed {
                            Ok(_) => AttemptOutcome::Answered,
                            Err(error) => AttemptOutcome::from_error(error),
                        };
                        attempts.extend(call_attempts(
                            counting.take(),
                            ProviderKind::Crossref,
                            &[AttemptPurpose::ReferenceUnion],
                            |_| outcome.clone(),
                        ));
                        match listed {
                            Ok(list) => {
                                if merge_references(&mut fetched.work.references, list) {
                                    contributions.push(ProviderKind::Crossref);
                                }
                            }
                            // The helper found no list. That adds nothing and breaks nothing.
                            Err(ProviderFetchError::NotFound { .. }) => {}
                            // The work cache is permanent, so a list that a transient failure
                            // left short is retried instead of stored.
                            Err(
                                ProviderFetchError::Retryable { message, .. }
                                | ProviderFetchError::ReferenceRetryable { message, .. },
                            ) => {
                                return RouteOutput {
                                    attempts,
                                    result: Err(RouteError::Retryable { message }),
                                    references: None,
                                };
                            }
                            Err(_) => helper_fatal = true,
                        }
                    }
                    let summary = ReferenceSummary::new(&fetched.references, helper_fatal);
                    let fetched = stamp_provenance(fetched, &attempts, &contributions, &summary);
                    return RouteOutput {
                        attempts,
                        result: Ok(fetched),
                        references: Some(summary),
                    };
                }
                Err(error) => {
                    let (metadata, references) = match &error {
                        ProviderFetchError::ReferenceRetryable { .. }
                        | ProviderFetchError::ReferenceFatal { .. } => {
                            (AttemptOutcome::Answered, AttemptOutcome::from_error(&error))
                        }
                        _ => (AttemptOutcome::from_error(&error), AttemptOutcome::Answered),
                    };
                    let reference_failed = references != AttemptOutcome::Answered;
                    let always: &[AttemptPurpose] = if reference_failed {
                        &[
                            AttemptPurpose::Metadata,
                            AttemptPurpose::ReferenceResolution,
                        ]
                    } else {
                        &[AttemptPurpose::Metadata]
                    };
                    attempts.extend(call_attempts(
                        counts,
                        kind,
                        always,
                        |purpose| match purpose {
                            AttemptPurpose::Metadata => metadata.clone(),
                            AttemptPurpose::ReferenceResolution => references.clone(),
                            _ => AttemptOutcome::Answered,
                        },
                    ));
                    match error {
                        ProviderFetchError::NotFound { .. } => {}
                        ProviderFetchError::Rejected { reason, .. } => {
                            return RouteOutput {
                                attempts,
                                result: Err(RouteError::NotFound {
                                    reason: Some(reason),
                                }),
                                references: None,
                            };
                        }
                        ProviderFetchError::Retryable { message, .. } => {
                            last_retryable = Some(message);
                        }
                        ProviderFetchError::Fatal { message, .. } => {
                            last_fatal = Some(message);
                        }
                        ProviderFetchError::ReferenceRetryable { message, .. } => {
                            return RouteOutput {
                                attempts,
                                result: Err(RouteError::Retryable { message }),
                                references: None,
                            };
                        }
                        ProviderFetchError::ReferenceFatal { message, .. } => {
                            return RouteOutput {
                                attempts,
                                result: Err(RouteError::Fatal { message }),
                                references: None,
                            };
                        }
                    }
                }
            }
        }
        let error = match (last_retryable, last_fatal) {
            (Some(message), _) => RouteError::Retryable { message },
            (None, Some(message)) => RouteError::Fatal { message },
            (None, None) => RouteError::NotFound { reason: None },
        };
        RouteOutput {
            attempts,
            result: Err(error),
            references: None,
        }
    }
}

/// Merges Crossref's reference list into the answer's references and says whether it added
/// anything. The answer's order is kept. A DOI both lists name keeps the answer's position
/// and takes Crossref's bibliographic fields, because the answer carries only the DOI.
/// Entries without a DOI cannot become child jobs, and they follow as unresolved references.
fn merge_references(references: &mut Vec<Reference>, list: Vec<Reference>) -> bool {
    let mut position: HashMap<String, usize> = references
        .iter()
        .enumerate()
        .filter_map(|(index, reference)| {
            let doi = normalize_doi(reference.doi.as_deref()?).ok()?;
            Some((doi, index))
        })
        .collect();
    let mut seen_in_list = HashSet::new();
    let mut contributed = false;
    for mut reference in list {
        let Some(doi) = reference
            .doi
            .as_deref()
            .and_then(|doi| normalize_doi(doi).ok())
        else {
            references.push(Reference {
                doi: None,
                ..reference
            });
            contributed = true;
            continue;
        };
        if !seen_in_list.insert(doi.clone()) {
            continue;
        }
        reference.doi = Some(doi.clone());
        match position.get(&doi) {
            Some(&index) => references[index] = reference,
            None => {
                position.insert(doi, references.len());
                references.push(reference);
            }
        }
        contributed = true;
    }
    contributed
}

/// The chain one job runs. The stored chain keeps its order and drops providers that
/// are not enabled. When nothing stored is enabled, the job runs the enabled chain.
pub fn effective_chain(stored: &[ProviderKind], enabled: &[ProviderKind]) -> Vec<ProviderKind> {
    let filtered = unique(stored.iter().copied().filter(|kind| enabled.contains(kind)));
    if filtered.is_empty() {
        enabled.to_vec()
    } else {
        filtered
    }
}

/// The reference sources of a work, in the order they contributed, each listed once.
pub fn citation_sources(contributions: &[ProviderKind]) -> Vec<ProviderKind> {
    unique(contributions.iter().copied())
}

fn unique(kinds: impl IntoIterator<Item = ProviderKind>) -> Vec<ProviderKind> {
    let mut seen = Vec::new();
    for kind in kinds {
        if !seen.contains(&kind) {
            seen.push(kind);
        }
    }
    seen
}

/// Records the answering provider, its attempts, the reference status and the reference
/// sources on the work and in its raw payload. `contributions` lists the providers whose
/// references were kept, in the order they contributed, and is empty when none were.
fn stamp_provenance(
    mut fetched: FetchedWork,
    attempts: &[ProviderAttempt],
    contributions: &[ProviderKind],
    summary: &ReferenceSummary,
) -> FetchedWork {
    let provider = fetched.provider;
    let sources = citation_sources(contributions);
    let source_names: Vec<&str> = sources.iter().map(|kind| kind.as_str()).collect();
    let mut raw = match std::mem::take(&mut fetched.work.raw) {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    raw.insert(
        "deepref_metadata_source".to_owned(),
        json!(provider.as_str()),
    );
    raw.insert("deepref_provider_attempts".to_owned(), json!(attempts));
    raw.insert("deepref_citation_sources".to_owned(), json!(source_names));
    raw.insert("deepref_reference_status".to_owned(), json!(summary));
    fetched.work.raw = Value::Object(raw);
    fetched.work.work.metadata_provider = provider.as_str().to_owned();
    fetched.work.work.citation_provider = source_names.join(",");
    fetched
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use chrono::Utc;
    use deepref_domain::{FetchStatus, Work, WorkWithReferences};
    use futures::executor::block_on;

    use super::*;
    use crate::acquisition::{BudgetStop, PermitError, ProviderFuture};
    use deepref_domain::ProviderKind as Kind;

    const DOI: &str = "10.1000/example";

    /// Adapter calls in the order they ran, shared by every fake in one router.
    type Calls = Arc<Mutex<Vec<ProviderKind>>>;
    /// Each lookup that ran, with the providers it was told had already tried the DOI.
    type Tried = Arc<Mutex<Vec<(ProviderKind, Vec<ProviderKind>)>>>;

    struct FakeProvider {
        kind: ProviderKind,
        answer: Result<WorkWithReferences, ProviderFetchError>,
        report: ReferenceReport,
        list: Result<Vec<Reference>, ProviderFetchError>,
        unions: bool,
        /// Extra requests the adapter sends after the router's first permit.
        sends: Vec<OutboundRequest>,
        calls: Calls,
        tried: Tried,
    }

    impl WorkProvider for FakeProvider {
        fn kind(&self) -> ProviderKind {
            self.kind
        }

        fn fetch_work_with_references<'a>(
            &'a self,
            _doi: &'a str,
            permit: &'a dyn ProviderPermit,
        ) -> ProviderFuture<'a, FetchedWork, ProviderFetchError> {
            Box::pin(async move {
                self.calls.lock().expect("call log").push(self.kind);
                for request in &self.sends {
                    permit.acquire(*request).await.map_err(|error| {
                        ProviderFetchError::Retryable {
                            provider: self.kind,
                            message: error.to_string(),
                        }
                    })?;
                }
                self.answer.clone().map(|work| FetchedWork {
                    work,
                    provider: self.kind,
                    references: self.report.clone(),
                })
            })
        }

        fn fetch_work_after<'a>(
            &'a self,
            doi: &'a str,
            tried: &'a [ProviderKind],
            permit: &'a dyn ProviderPermit,
        ) -> ProviderFuture<'a, FetchedWork, ProviderFetchError> {
            self.tried
                .lock()
                .expect("tried log")
                .push((self.kind, tried.to_vec()));
            self.fetch_work_with_references(doi, permit)
        }

        fn reference_list<'a>(
            &'a self,
            _doi: &'a str,
            permit: &'a dyn ProviderPermit,
        ) -> ProviderFuture<'a, Vec<Reference>, ProviderFetchError> {
            Box::pin(async move {
                permit
                    .acquire(OutboundRequest::new(
                        self.kind,
                        AttemptPurpose::ReferenceUnion,
                    ))
                    .await
                    .map_err(|error| ProviderFetchError::Retryable {
                        provider: self.kind,
                        message: error.to_string(),
                    })?;
                self.list.clone()
            })
        }

        fn unions_crossref_references(&self) -> bool {
            self.unions
        }
    }

    #[derive(Default)]
    struct FakePermit {
        requested: Mutex<Vec<OutboundRequest>>,
        unavailable: bool,
    }

    impl ProviderPermit for FakePermit {
        fn acquire<'a>(&'a self, request: OutboundRequest) -> ProviderFuture<'a, (), PermitError> {
            Box::pin(async move {
                self.requested.lock().expect("permit log").push(request);
                if self.unavailable {
                    Err(PermitError("limiter is down".to_owned()))
                } else {
                    Ok(())
                }
            })
        }
    }

    fn work() -> WorkWithReferences {
        WorkWithReferences {
            work: Work {
                doi: DOI.to_owned(),
                title: Some("A title".to_owned()),
                abstract_text: None,
                work_type: None,
                publisher: None,
                container_title: None,
                issued_year: Some(2024),
                published_year: None,
                url: None,
                total_citations: 0,
                references_count: 0,
                metadata_provider: "fake".to_owned(),
                citation_provider: "fake".to_owned(),
                fetched_at: Utc::now(),
                fetch_status: FetchStatus::Fetched.as_str().to_owned(),
            },
            references: Vec::new(),
            raw: json!({ "title": ["A title"] }),
        }
    }

    fn answers() -> Result<WorkWithReferences, ProviderFetchError> {
        Ok(work())
    }

    fn not_found(provider: ProviderKind) -> Result<WorkWithReferences, ProviderFetchError> {
        Err(ProviderFetchError::NotFound {
            provider,
            doi: DOI.to_owned(),
            reason: None,
        })
    }

    fn rejected(
        provider: ProviderKind,
        reason: &str,
    ) -> Result<WorkWithReferences, ProviderFetchError> {
        Err(ProviderFetchError::Rejected {
            provider,
            reason: reason.to_owned(),
        })
    }

    fn retryable(
        provider: ProviderKind,
        message: &str,
    ) -> Result<WorkWithReferences, ProviderFetchError> {
        Err(ProviderFetchError::Retryable {
            provider,
            message: message.to_owned(),
        })
    }

    fn fatal(
        provider: ProviderKind,
        message: &str,
    ) -> Result<WorkWithReferences, ProviderFetchError> {
        Err(ProviderFetchError::Fatal {
            provider,
            message: message.to_owned(),
        })
    }

    fn provider(
        kind: ProviderKind,
        answer: Result<WorkWithReferences, ProviderFetchError>,
        calls: &Calls,
        tried: &Tried,
    ) -> FakeProvider {
        FakeProvider {
            kind,
            answer,
            report: ReferenceReport::default(),
            list: Ok(Vec::new()),
            unions: false,
            sends: vec![OutboundRequest::new(kind, AttemptPurpose::Metadata)],
            calls: Arc::clone(calls),
            tried: Arc::clone(tried),
        }
    }

    fn fake(
        kind: ProviderKind,
        answer: Result<WorkWithReferences, ProviderFetchError>,
        calls: &Calls,
    ) -> Box<dyn WorkProvider> {
        boxed(provider(kind, answer, calls, &Tried::default()))
    }

    fn boxed(provider: FakeProvider) -> Box<dyn WorkProvider> {
        Box::new(provider)
    }

    fn reference(doi: Option<&str>) -> Reference {
        Reference {
            doi: doi.map(str::to_owned),
            key: None,
            raw_unstructured: None,
            article_title: None,
            author: None,
            year: None,
            volume: None,
            first_page: None,
        }
    }

    fn work_citing(references: Vec<Reference>) -> WorkWithReferences {
        WorkWithReferences {
            references,
            ..work()
        }
    }

    fn reference_dois(fetched: &FetchedWork) -> Vec<String> {
        fetched
            .work
            .references
            .iter()
            .filter_map(|reference| reference.doi.clone())
            .collect()
    }

    fn router(deployment: &[ProviderKind], adapters: Vec<Box<dyn WorkProvider>>) -> ProviderRouter {
        ProviderRouter::new(deployment.to_vec(), adapters).expect("router should build")
    }

    fn run(router: &ProviderRouter, stored: &[ProviderKind], permit: &FakePermit) -> RouteOutput {
        block_on(router.fetch_work(stored, DOI, permit))
    }

    fn answered_by(output: &RouteOutput) -> Option<ProviderKind> {
        output.result.as_ref().ok().map(|fetched| fetched.provider)
    }

    fn call_log(calls: &Calls) -> Vec<ProviderKind> {
        calls.lock().expect("call log").clone()
    }

    fn requested_services(permit: &FakePermit) -> Vec<OutboundService> {
        permit
            .requested
            .lock()
            .expect("permit log")
            .iter()
            .map(|request| request.service)
            .collect()
    }

    fn outcomes(output: &RouteOutput) -> Vec<(ProviderKind, AttemptOutcome)> {
        output
            .attempts
            .iter()
            .filter(|attempt| attempt.purpose == AttemptPurpose::Metadata)
            .map(|attempt| {
                (
                    attempt.provider.provider().expect("a provider"),
                    attempt.outcome.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn a_not_found_moves_on_to_the_next_provider() {
        let calls = Calls::default();
        let router = router(
            &[ProviderKind::OpenAlex, ProviderKind::Crossref],
            vec![
                fake(
                    ProviderKind::OpenAlex,
                    not_found(ProviderKind::OpenAlex),
                    &calls,
                ),
                fake(ProviderKind::Crossref, answers(), &calls),
            ],
        );

        let output = run(&router, &[], &FakePermit::default());

        assert_eq!(answered_by(&output), Some(ProviderKind::Crossref));
        assert_eq!(
            outcomes(&output),
            vec![
                (
                    ProviderKind::OpenAlex,
                    AttemptOutcome::NotFound { reason: None }
                ),
                (ProviderKind::Crossref, AttemptOutcome::Answered),
            ]
        );
    }

    #[test]
    fn a_doi_only_datacite_answers_falls_through_to_datacite_with_its_provenance() {
        let calls = Calls::default();
        let router = router(
            &[
                ProviderKind::OpenAlex,
                ProviderKind::Crossref,
                ProviderKind::DataCite,
            ],
            vec![
                fake(
                    ProviderKind::OpenAlex,
                    not_found(ProviderKind::OpenAlex),
                    &calls,
                ),
                fake(
                    ProviderKind::Crossref,
                    not_found(ProviderKind::Crossref),
                    &calls,
                ),
                fake(ProviderKind::DataCite, answers(), &calls),
            ],
        );

        let output = run(&router, &[], &FakePermit::default());

        assert_eq!(answered_by(&output), Some(ProviderKind::DataCite));
        assert_eq!(
            outcomes(&output),
            vec![
                (
                    ProviderKind::OpenAlex,
                    AttemptOutcome::NotFound { reason: None }
                ),
                (
                    ProviderKind::Crossref,
                    AttemptOutcome::NotFound { reason: None }
                ),
                (ProviderKind::DataCite, AttemptOutcome::Answered),
            ]
        );
        assert_eq!(
            call_log(&calls),
            [
                ProviderKind::OpenAlex,
                ProviderKind::Crossref,
                ProviderKind::DataCite
            ]
        );
        let fetched = output.result.expect("DataCite should answer");
        assert_eq!(fetched.work.work.metadata_provider, "datacite");
        assert_eq!(fetched.work.work.citation_provider, "");
        assert_eq!(fetched.work.raw["deepref_metadata_source"], "datacite");
    }

    #[test]
    fn a_rejected_answer_ends_the_chain_as_not_found_with_its_reason() {
        let calls = Calls::default();
        let router = router(
            &[
                ProviderKind::OpenAlex,
                ProviderKind::Crossref,
                ProviderKind::DataCite,
            ],
            vec![
                fake(
                    ProviderKind::OpenAlex,
                    rejected(ProviderKind::OpenAlex, "paratext"),
                    &calls,
                ),
                fake(ProviderKind::Crossref, answers(), &calls),
                fake(ProviderKind::DataCite, answers(), &calls),
            ],
        );

        let output = run(&router, &[], &FakePermit::default());

        assert_eq!(
            output.result.as_ref().err().cloned(),
            Some(RouteError::NotFound {
                reason: Some("paratext".to_owned()),
            })
        );
        assert_eq!(
            outcomes(&output),
            vec![(
                ProviderKind::OpenAlex,
                AttemptOutcome::Rejected {
                    reason: "paratext".to_owned(),
                },
            )]
        );
        assert_eq!(call_log(&calls), vec![ProviderKind::OpenAlex]);
    }

    #[test]
    fn a_retryable_failure_moves_on_to_the_next_provider() {
        let calls = Calls::default();
        let router = router(
            &[ProviderKind::OpenAlex, ProviderKind::Crossref],
            vec![
                fake(
                    ProviderKind::OpenAlex,
                    retryable(ProviderKind::OpenAlex, "503 from upstream"),
                    &calls,
                ),
                fake(ProviderKind::Crossref, answers(), &calls),
            ],
        );

        let output = run(&router, &[], &FakePermit::default());

        assert_eq!(answered_by(&output), Some(ProviderKind::Crossref));
        assert_eq!(
            outcomes(&output),
            vec![
                (
                    ProviderKind::OpenAlex,
                    AttemptOutcome::Retryable {
                        message: "503 from upstream".to_owned(),
                    },
                ),
                (ProviderKind::Crossref, AttemptOutcome::Answered),
            ]
        );
    }

    #[test]
    fn a_fatal_failure_is_recorded_and_the_chain_moves_on() {
        let calls = Calls::default();
        let router = router(
            &[ProviderKind::OpenAlex, ProviderKind::Crossref],
            vec![
                fake(
                    ProviderKind::OpenAlex,
                    fatal(ProviderKind::OpenAlex, "bad request"),
                    &calls,
                ),
                fake(ProviderKind::Crossref, answers(), &calls),
            ],
        );

        let output = run(&router, &[], &FakePermit::default());

        assert_eq!(answered_by(&output), Some(ProviderKind::Crossref));
        assert_eq!(
            outcomes(&output),
            vec![
                (
                    ProviderKind::OpenAlex,
                    AttemptOutcome::Fatal {
                        message: "bad request".to_owned(),
                    },
                ),
                (ProviderKind::Crossref, AttemptOutcome::Answered),
            ]
        );
    }

    #[test]
    fn fatal_failures_with_no_answer_end_fatal_after_every_provider_was_asked() {
        let calls = Calls::default();
        let router = router(
            &[
                ProviderKind::OpenAlex,
                ProviderKind::Crossref,
                ProviderKind::DataCite,
            ],
            vec![
                fake(
                    ProviderKind::OpenAlex,
                    not_found(ProviderKind::OpenAlex),
                    &calls,
                ),
                fake(
                    ProviderKind::Crossref,
                    fatal(ProviderKind::Crossref, "bad request"),
                    &calls,
                ),
                fake(
                    ProviderKind::DataCite,
                    not_found(ProviderKind::DataCite),
                    &calls,
                ),
            ],
        );

        let output = run(&router, &[], &FakePermit::default());

        assert_eq!(
            output.result.as_ref().err().cloned(),
            Some(RouteError::Fatal {
                message: "bad request".to_owned(),
            })
        );
        assert_eq!(call_log(&calls).len(), 3);
    }

    #[test]
    fn a_retryable_failure_outranks_a_fatal_one_when_nothing_answers() {
        let calls = Calls::default();
        let router = router(
            &[ProviderKind::OpenAlex, ProviderKind::Crossref],
            vec![
                fake(
                    ProviderKind::OpenAlex,
                    fatal(ProviderKind::OpenAlex, "bad request"),
                    &calls,
                ),
                fake(
                    ProviderKind::Crossref,
                    retryable(ProviderKind::Crossref, "503 from upstream"),
                    &calls,
                ),
            ],
        );

        let output = run(&router, &[], &FakePermit::default());

        assert_eq!(
            output.result.as_ref().err().cloned(),
            Some(RouteError::Retryable {
                message: "503 from upstream".to_owned(),
            })
        );
        assert_eq!(output.attempts.len(), 2);
    }

    #[test]
    fn every_provider_not_found_is_not_found() {
        let calls = Calls::default();
        let router = router(
            &[ProviderKind::OpenAlex, ProviderKind::Crossref],
            vec![
                fake(
                    ProviderKind::OpenAlex,
                    not_found(ProviderKind::OpenAlex),
                    &calls,
                ),
                fake(
                    ProviderKind::Crossref,
                    not_found(ProviderKind::Crossref),
                    &calls,
                ),
            ],
        );

        let output = run(&router, &[], &FakePermit::default());

        assert_eq!(
            output.result.as_ref().err().cloned(),
            Some(RouteError::NotFound { reason: None })
        );
        assert_eq!(output.attempts.len(), 2);
    }

    #[test]
    fn a_retryable_failure_without_an_answer_is_retryable() {
        let calls = Calls::default();
        let router = router(
            &[ProviderKind::OpenAlex, ProviderKind::Crossref],
            vec![
                fake(
                    ProviderKind::OpenAlex,
                    retryable(ProviderKind::OpenAlex, "upstream timed out"),
                    &calls,
                ),
                fake(
                    ProviderKind::Crossref,
                    not_found(ProviderKind::Crossref),
                    &calls,
                ),
            ],
        );

        let output = run(&router, &[], &FakePermit::default());

        assert_eq!(
            output.result.as_ref().err().cloned(),
            Some(RouteError::Retryable {
                message: "upstream timed out".to_owned(),
            })
        );
    }

    #[test]
    fn a_stored_chain_with_nothing_enabled_falls_back_to_the_deployment_chain() {
        let calls = Calls::default();
        let router = router(
            &[ProviderKind::Crossref],
            vec![fake(ProviderKind::Crossref, answers(), &calls)],
        );

        let output = run(&router, &[ProviderKind::OpenAlex], &FakePermit::default());

        assert_eq!(answered_by(&output), Some(ProviderKind::Crossref));
        assert_eq!(call_log(&calls), vec![ProviderKind::Crossref]);
    }

    #[test]
    fn the_stored_chain_is_filtered_to_the_allowlist_and_keeps_its_order() {
        let calls = Calls::default();
        let router = router(
            &[ProviderKind::OpenAlex, ProviderKind::Crossref],
            vec![
                fake(ProviderKind::OpenAlex, answers(), &calls),
                fake(
                    ProviderKind::Crossref,
                    not_found(ProviderKind::Crossref),
                    &calls,
                ),
                fake(ProviderKind::DataCite, answers(), &calls),
            ],
        );
        let stored = [
            ProviderKind::DataCite,
            ProviderKind::Crossref,
            ProviderKind::OpenAlex,
        ];

        let output = run(&router, &stored, &FakePermit::default());

        assert_eq!(answered_by(&output), Some(ProviderKind::OpenAlex));
        assert_eq!(
            call_log(&calls),
            vec![ProviderKind::Crossref, ProviderKind::OpenAlex]
        );
    }

    #[test]
    fn the_permit_is_taken_once_per_attempt() {
        let calls = Calls::default();
        let router = router(
            &[
                ProviderKind::OpenAlex,
                ProviderKind::Crossref,
                ProviderKind::DataCite,
            ],
            vec![
                fake(
                    ProviderKind::OpenAlex,
                    retryable(ProviderKind::OpenAlex, "slow"),
                    &calls,
                ),
                fake(
                    ProviderKind::Crossref,
                    not_found(ProviderKind::Crossref),
                    &calls,
                ),
                fake(ProviderKind::DataCite, answers(), &calls),
            ],
        );
        let permit = FakePermit::default();

        run(&router, &[], &permit);

        assert_eq!(
            requested_services(&permit),
            vec![
                OutboundService::Provider(ProviderKind::OpenAlex),
                OutboundService::Provider(ProviderKind::Crossref),
                OutboundService::Provider(ProviderKind::DataCite),
            ]
        );
    }

    #[test]
    fn a_permit_that_cannot_be_taken_is_a_retryable_failure_with_no_request_sent() {
        let calls = Calls::default();
        let router = router(
            &[ProviderKind::Crossref],
            vec![fake(ProviderKind::Crossref, answers(), &calls)],
        );
        let permit = FakePermit {
            unavailable: true,
            ..FakePermit::default()
        };

        let output = run(&router, &[], &permit);

        assert_eq!(
            output.result.as_ref().err().cloned(),
            Some(RouteError::Retryable {
                message: "provider permit unavailable: limiter is down".to_owned(),
            })
        );
    }

    #[test]
    fn a_success_records_its_provenance_on_the_work_and_in_raw() {
        let calls = Calls::default();
        let router = router(
            &[ProviderKind::OpenAlex, ProviderKind::Crossref],
            vec![
                fake(
                    ProviderKind::OpenAlex,
                    not_found(ProviderKind::OpenAlex),
                    &calls,
                ),
                fake(
                    ProviderKind::Crossref,
                    Ok(work_citing(vec![reference(Some("10.1000/a"))])),
                    &calls,
                ),
            ],
        );

        let output = run(&router, &[], &FakePermit::default());
        let fetched = output.result.expect("crossref should answer");

        assert_eq!(fetched.work.work.metadata_provider, "crossref");
        assert_eq!(fetched.work.work.citation_provider, "crossref");
        assert_eq!(fetched.work.raw["title"], json!(["A title"]));
        assert_eq!(
            fetched.work.raw["deepref_metadata_source"],
            json!("crossref")
        );
        assert_eq!(
            fetched.work.raw["deepref_citation_sources"],
            json!(["crossref"])
        );
        assert_eq!(
            fetched.work.raw["deepref_provider_attempts"],
            json!([
                { "provider": "openalex", "purpose": "metadata", "requests": 1, "outcome": "not_found" },
                { "provider": "crossref", "purpose": "metadata", "requests": 1, "outcome": "answered" },
            ])
        );
    }

    #[test]
    fn the_effective_chain_keeps_stored_order_drops_repeats_and_falls_back_when_empty() {
        let enabled = [ProviderKind::OpenAlex, ProviderKind::Crossref];

        assert_eq!(
            effective_chain(
                &[
                    ProviderKind::Crossref,
                    ProviderKind::Crossref,
                    ProviderKind::OpenAlex,
                ],
                &enabled,
            ),
            vec![ProviderKind::Crossref, ProviderKind::OpenAlex]
        );
        assert_eq!(
            effective_chain(&[ProviderKind::DataCite], &enabled),
            enabled.to_vec()
        );
        assert_eq!(effective_chain(&[], &enabled), enabled.to_vec());
    }

    #[test]
    fn citation_sources_list_each_contribution_once_in_order() {
        assert_eq!(
            citation_sources(&[
                ProviderKind::OpenAlex,
                ProviderKind::Crossref,
                ProviderKind::OpenAlex,
            ]),
            vec![ProviderKind::OpenAlex, ProviderKind::Crossref]
        );
    }

    #[test]
    fn the_outcome_name_is_the_serialized_tag() {
        let outcomes = [
            AttemptOutcome::Answered,
            AttemptOutcome::BudgetExhausted {
                reset_secs: Some(60),
            },
            AttemptOutcome::NotFound { reason: None },
            AttemptOutcome::Rejected {
                reason: "paratext".to_owned(),
            },
            AttemptOutcome::Retryable {
                message: "busy".to_owned(),
            },
            AttemptOutcome::Fatal {
                message: "blocked".to_owned(),
            },
        ];
        for outcome in outcomes {
            let serialized = serde_json::to_value(&outcome).expect("outcome serializes");
            assert_eq!(serialized["outcome"], outcome.name());
        }
    }

    #[test]
    fn the_router_refuses_a_chain_with_nothing_enabled_and_repeated_adapters() {
        let calls = Calls::default();

        assert_eq!(
            ProviderRouter::new(
                vec![ProviderKind::DataCite],
                vec![fake(ProviderKind::Crossref, answers(), &calls)],
            )
            .err(),
            Some(RouterError::NoEnabledProvider)
        );
        assert_eq!(
            ProviderRouter::new(
                vec![ProviderKind::Crossref],
                vec![
                    fake(ProviderKind::Crossref, answers(), &calls),
                    fake(ProviderKind::Crossref, answers(), &calls),
                ],
            )
            .err(),
            Some(RouterError::DuplicateAdapter(ProviderKind::Crossref))
        );
    }

    #[test]
    fn a_fall_through_tells_the_next_provider_which_providers_already_tried_the_doi() {
        let calls = Calls::default();
        let tried = Tried::default();
        let router = router(
            &[ProviderKind::OpenAlex, ProviderKind::Crossref],
            vec![
                boxed(provider(
                    ProviderKind::OpenAlex,
                    not_found(ProviderKind::OpenAlex),
                    &calls,
                    &tried,
                )),
                boxed(provider(ProviderKind::Crossref, answers(), &calls, &tried)),
            ],
        );

        let output = run(&router, &[], &FakePermit::default());

        assert_eq!(answered_by(&output), Some(ProviderKind::Crossref));
        assert_eq!(
            tried.lock().expect("tried log").clone(),
            vec![
                (ProviderKind::OpenAlex, vec![]),
                (ProviderKind::Crossref, vec![ProviderKind::OpenAlex]),
            ]
        );
    }
    fn cited(doi: &str, title: &str) -> Reference {
        Reference {
            article_title: Some(title.to_owned()),
            ..reference(Some(doi))
        }
    }

    /// An OpenAlex answer that unions Crossref, with `openalex_refs` as its references and
    /// `report` as what its resolution lost. Crossref's reference list is `list`.
    fn union_run(
        deployment: &[ProviderKind],
        stored: &[ProviderKind],
        openalex_refs: Vec<Reference>,
        report: ReferenceReport,
        list: Result<Vec<Reference>, ProviderFetchError>,
    ) -> (RouteOutput, FakePermit, Calls) {
        let calls = Calls::default();
        let tried = Tried::default();
        let openalex = FakeProvider {
            report,
            unions: true,
            ..provider(
                ProviderKind::OpenAlex,
                Ok(work_citing(openalex_refs)),
                &calls,
                &tried,
            )
        };
        let crossref = FakeProvider {
            list,
            ..provider(ProviderKind::Crossref, answers(), &calls, &tried)
        };
        let router = router(deployment, vec![boxed(openalex), boxed(crossref)]);
        let permit = FakePermit::default();
        let output = run(&router, stored, &permit);
        (output, permit, calls)
    }

    const BOTH: [ProviderKind; 2] = [ProviderKind::OpenAlex, ProviderKind::Crossref];

    fn status(fetched: &FetchedWork) -> &Value {
        &fetched.work.raw["deepref_reference_status"]
    }

    fn crossref_failure(error: ProviderFetchError) -> Result<Vec<Reference>, ProviderFetchError> {
        Err(error)
    }

    #[test]
    fn an_attempt_records_its_purpose_and_the_permits_it_was_granted() {
        let calls = Calls::default();
        let openalex = FakeProvider {
            sends: vec![
                OutboundRequest::new(Kind::OpenAlex, AttemptPurpose::Metadata),
                OutboundRequest::new(Kind::OpenAlex, AttemptPurpose::ReferenceResolution),
                OutboundRequest::new(Kind::OpenAlex, AttemptPurpose::ReferenceResolution),
            ],
            ..provider(ProviderKind::OpenAlex, answers(), &calls, &Tried::default())
        };
        let router = router(&[ProviderKind::OpenAlex], vec![boxed(openalex)]);

        let output = run(&router, &[], &FakePermit::default());

        let seen: Vec<_> = output
            .attempts
            .iter()
            .map(|attempt| (attempt.purpose, attempt.requests))
            .collect();
        assert_eq!(
            seen,
            vec![
                (AttemptPurpose::Metadata, 1),
                (AttemptPurpose::ReferenceResolution, 2)
            ]
        );
    }

    #[test]
    fn a_refused_permit_leaves_an_attempt_with_no_requests() {
        let calls = Calls::default();
        let router = router(
            &[ProviderKind::Crossref],
            vec![fake(ProviderKind::Crossref, answers(), &calls)],
        );
        let permit = FakePermit {
            unavailable: true,
            ..FakePermit::default()
        };

        let output = run(&router, &[], &permit);

        assert_eq!(output.attempts.len(), 1);
        assert_eq!(output.attempts[0].requests, 0);
        assert_eq!(output.attempts[0].purpose, AttemptPurpose::Metadata);
    }

    #[test]
    fn a_reference_failure_after_the_metadata_answered_never_asks_the_next_provider() {
        let calls = Calls::default();
        let router = router(
            &BOTH,
            vec![
                fake(
                    ProviderKind::OpenAlex,
                    Err(ProviderFetchError::ReferenceRetryable {
                        provider: ProviderKind::OpenAlex,
                        message: "502 on the second batch".to_owned(),
                    }),
                    &calls,
                ),
                fake(ProviderKind::Crossref, answers(), &calls),
            ],
        );

        let output = run(&router, &[], &FakePermit::default());

        assert_eq!(
            output.result.as_ref().err().cloned(),
            Some(RouteError::Retryable {
                message: "502 on the second batch".to_owned(),
            })
        );
        assert_eq!(call_log(&calls), vec![ProviderKind::OpenAlex]);
        let seen: Vec<_> = output
            .attempts
            .iter()
            .map(|attempt| (attempt.purpose, attempt.outcome.name()))
            .collect();
        assert_eq!(
            seen,
            vec![
                (AttemptPurpose::Metadata, "answered"),
                (AttemptPurpose::ReferenceResolution, "retryable"),
            ]
        );
    }

    #[test]
    fn a_fatal_reference_failure_after_the_metadata_answered_fails_the_fetch() {
        let calls = Calls::default();
        let router = router(
            &BOTH,
            vec![
                fake(
                    ProviderKind::OpenAlex,
                    Err(ProviderFetchError::ReferenceFatal {
                        provider: ProviderKind::OpenAlex,
                        message: "the list has no results array".to_owned(),
                    }),
                    &calls,
                ),
                fake(ProviderKind::Crossref, answers(), &calls),
            ],
        );

        let output = run(&router, &[], &FakePermit::default());

        assert_eq!(
            output.result.as_ref().err().cloned(),
            Some(RouteError::Fatal {
                message: "the list has no results array".to_owned(),
            })
        );
        assert_eq!(call_log(&calls), vec![ProviderKind::OpenAlex]);
    }

    #[test]
    fn the_union_runs_on_every_openalex_answer_even_when_the_stored_chain_names_only_openalex() {
        let (output, permit, calls) = union_run(
            &BOTH,
            &[ProviderKind::OpenAlex],
            vec![cited("10.1000/a", "A")],
            ReferenceReport::default(),
            Ok(vec![cited("10.1000/b", "B")]),
        );

        let fetched = output.result.expect("openalex should answer");
        assert_eq!(reference_dois(&fetched), ["10.1000/a", "10.1000/b"]);
        assert_eq!(call_log(&calls), vec![ProviderKind::OpenAlex]);
        assert!(
            permit
                .requested
                .lock()
                .expect("permit log")
                .contains(&OutboundRequest::new(
                    Kind::Crossref,
                    AttemptPurpose::ReferenceUnion
                ))
        );
        assert_eq!(fetched.work.work.citation_provider, "openalex,crossref");
    }

    #[test]
    fn the_union_runs_when_every_openalex_target_resolved_and_when_the_list_is_empty() {
        for refs in [vec![cited("10.1000/a", "A")], Vec::new()] {
            let (output, permit, _) =
                union_run(&BOTH, &[], refs, ReferenceReport::default(), Ok(Vec::new()));
            assert!(output.result.is_ok());
            assert_eq!(
                requested_services(&permit)
                    .iter()
                    .filter(|service| **service == Kind::Crossref)
                    .count(),
                1
            );
        }
    }

    #[test]
    fn the_union_does_not_run_when_crossref_is_not_enabled_in_the_deployment() {
        let (output, permit, _) = union_run(
            &[ProviderKind::OpenAlex],
            &[],
            vec![cited("10.1000/a", "A")],
            ReferenceReport::default(),
            Ok(vec![cited("10.1000/b", "B")]),
        );

        let fetched = output.result.expect("openalex should answer");
        assert_eq!(reference_dois(&fetched), ["10.1000/a"]);
        assert!(
            !requested_services(&permit)
                .iter()
                .any(|service| *service == Kind::Crossref)
        );
    }

    #[test]
    fn a_duplicate_doi_keeps_the_openalex_position_and_takes_the_crossref_bibliography() {
        let (output, _, _) = union_run(
            &BOTH,
            &[],
            vec![reference(Some("10.1000/a")), reference(Some("10.1000/b"))],
            ReferenceReport::default(),
            Ok(vec![
                cited("https://doi.org/10.1000/A", "Title of A"),
                cited("10.1000/c", "Title of C"),
            ]),
        );

        let fetched = output.result.expect("openalex should answer");
        assert_eq!(
            reference_dois(&fetched),
            ["10.1000/a", "10.1000/b", "10.1000/c"]
        );
        assert_eq!(
            fetched.work.references[0].article_title.as_deref(),
            Some("Title of A")
        );
        assert_eq!(fetched.work.work.citation_provider, "openalex,crossref");
    }

    #[test]
    fn crossref_entries_without_a_doi_join_the_references_as_unresolved() {
        let book = Reference {
            raw_unstructured: Some("Smith, J. A Book. 1999.".to_owned()),
            ..reference(None)
        };
        let (output, _, _) = union_run(
            &BOTH,
            &[],
            vec![reference(Some("10.1000/a"))],
            ReferenceReport::default(),
            Ok(vec![book.clone()]),
        );

        let fetched = output.result.expect("openalex should answer");
        let last = fetched.work.references.last().expect("a reference");
        assert_eq!(last.doi, None);
        assert_eq!(last.raw_unstructured, book.raw_unstructured);
        assert_eq!(fetched.work.work.citation_provider, "openalex,crossref");
    }

    #[test]
    fn a_provider_that_contributes_no_references_is_not_a_citation_source() {
        let (output, _, _) = union_run(
            &BOTH,
            &[],
            Vec::new(),
            ReferenceReport::default(),
            Ok(Vec::new()),
        );

        let fetched = output.result.expect("openalex should answer");
        assert_eq!(fetched.work.work.citation_provider, "");
        assert_eq!(fetched.work.raw["deepref_citation_sources"], json!([]));
        assert_eq!(status(&fetched)["status"], "complete");
    }

    #[test]
    fn only_crossref_is_the_citation_source_when_only_its_list_has_entries() {
        let (output, _, _) = union_run(
            &BOTH,
            &[],
            Vec::new(),
            ReferenceReport::default(),
            Ok(vec![reference(None)]),
        );

        let fetched = output.result.expect("openalex should answer");
        assert_eq!(fetched.work.work.citation_provider, "crossref");
    }

    #[test]
    fn a_transient_crossref_failure_retries_and_records_both_attempts() {
        let (output, _, _) = union_run(
            &BOTH,
            &[],
            vec![reference(Some("10.1000/a"))],
            ReferenceReport::default(),
            crossref_failure(ProviderFetchError::Retryable {
                provider: ProviderKind::Crossref,
                message: "503".to_owned(),
            }),
        );

        assert_eq!(
            output.result.as_ref().err().cloned(),
            Some(RouteError::Retryable {
                message: "503".to_owned()
            })
        );
        let seen: Vec<_> = output
            .attempts
            .iter()
            .map(|attempt| (attempt.provider, attempt.purpose, attempt.outcome.name()))
            .collect();
        assert_eq!(
            seen,
            vec![
                (
                    OutboundService::Provider(Kind::OpenAlex),
                    AttemptPurpose::Metadata,
                    "answered"
                ),
                (
                    OutboundService::Provider(Kind::Crossref),
                    AttemptPurpose::ReferenceUnion,
                    "retryable"
                ),
            ]
        );
    }

    #[test]
    fn a_fatal_crossref_failure_is_a_degraded_success_that_keeps_the_openalex_references() {
        let (output, _, _) = union_run(
            &BOTH,
            &[],
            vec![reference(Some("10.1000/a"))],
            ReferenceReport::default(),
            crossref_failure(ProviderFetchError::Fatal {
                provider: ProviderKind::Crossref,
                message: "malformed record".to_owned(),
            }),
        );

        let fetched = output.result.expect("the answer should be kept");
        assert_eq!(reference_dois(&fetched), ["10.1000/a"]);
        assert_eq!(status(&fetched)["status"], "degraded");
        assert_eq!(status(&fetched)["reasons"], json!(["helper_fatal"]));
        assert_eq!(fetched.work.work.citation_provider, "openalex");
        let union = output
            .attempts
            .iter()
            .find(|attempt| attempt.purpose == AttemptPurpose::ReferenceUnion)
            .expect("the union attempt is recorded");
        assert_eq!(union.outcome.name(), "fatal");
    }

    #[test]
    fn a_crossref_not_found_adds_nothing_and_keeps_an_existing_degradation() {
        let report = ReferenceReport {
            targets: 3,
            retained: 2,
            omitted: 1,
            ..ReferenceReport::default()
        };
        let (output, _, _) = union_run(
            &BOTH,
            &[],
            vec![reference(Some("10.1000/a"))],
            report,
            crossref_failure(ProviderFetchError::NotFound {
                provider: ProviderKind::Crossref,
                doi: DOI.to_owned(),
                reason: None,
            }),
        );

        let fetched = output.result.expect("openalex should answer");
        assert_eq!(status(&fetched)["status"], "degraded");
        assert_eq!(status(&fetched)["reasons"], json!(["omitted_targets"]));
        assert_eq!(status(&fetched)["omitted"], 1);
        let union = output
            .attempts
            .iter()
            .find(|attempt| attempt.purpose == AttemptPurpose::ReferenceUnion)
            .expect("the union attempt is recorded");
        assert_eq!(union.outcome.name(), "not_found");
    }

    #[test]
    fn a_budget_stop_is_recorded_with_its_reset_even_when_no_request_was_sent() {
        let report = ReferenceReport {
            targets: 250,
            retained: 100,
            unattempted: 150,
            budget_stop: Some(BudgetStop {
                reset_secs: Some(3_600),
            }),
            ..ReferenceReport::default()
        };
        let (output, _, _) = union_run(
            &BOTH,
            &[],
            vec![reference(Some("10.1000/a"))],
            report,
            Ok(Vec::new()),
        );

        let fetched = output.result.expect("openalex should answer");
        let budget = output
            .attempts
            .iter()
            .find(|attempt| attempt.purpose == AttemptPurpose::ReferenceResolution)
            .expect("the paused budget is in the history");
        assert_eq!(budget.requests, 0);
        assert_eq!(
            budget.outcome,
            AttemptOutcome::BudgetExhausted {
                reset_secs: Some(3_600)
            }
        );
        assert_eq!(status(&fetched)["reasons"], json!(["budget_exhausted"]));
        assert_eq!(status(&fetched)["budget_reset_secs"], 3_600);
        assert_eq!(status(&fetched)["unattempted"], 150);
    }

    #[test]
    fn omitted_and_doi_less_targets_degrade_the_status_with_separate_counts() {
        let report = ReferenceReport {
            targets: 101,
            retained: 10,
            omitted: 50,
            doi_less: 40,
            unattempted: 1,
            ..ReferenceReport::default()
        };
        let (output, _, _) = union_run(&BOTH, &[], Vec::new(), report, Ok(Vec::new()));

        let fetched = output.result.expect("openalex should answer");
        assert_eq!(
            status(&fetched)["reasons"],
            json!(["omitted_targets", "doi_less_targets"])
        );
        assert_eq!(status(&fetched)["retained"], 10);
        assert_eq!(status(&fetched)["omitted"], 50);
        assert_eq!(status(&fetched)["doi_less"], 40);
    }

    #[test]
    fn a_crossref_answer_does_not_union_with_itself() {
        let calls = Calls::default();
        let tried = Tried::default();
        let crossref = FakeProvider {
            unions: true,
            ..provider(ProviderKind::Crossref, answers(), &calls, &tried)
        };
        let router = router(&[ProviderKind::Crossref], vec![boxed(crossref)]);
        let permit = FakePermit::default();

        let output = run(&router, &[], &permit);

        assert!(output.result.is_ok());
        assert_eq!(permit.requested.lock().expect("permit log").len(), 1);
    }

    #[test]
    fn the_outcome_serializes_with_its_reset() {
        let value = serde_json::to_value(ProviderAttempt {
            provider: OutboundService::EuropePmc,
            purpose: AttemptPurpose::AbstractEnrichment,
            requests: 2,
            outcome: AttemptOutcome::BudgetExhausted { reset_secs: None },
        })
        .expect("attempt serializes");
        assert_eq!(value["provider"], "europepmc");
        assert_eq!(value["purpose"], "abstract_enrichment");
        assert_eq!(value["requests"], 2);
    }
}
