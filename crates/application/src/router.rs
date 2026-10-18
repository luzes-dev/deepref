//! Routes one DOI through the metadata provider chain: the allowlist, the decision
//! matrix and the provenance written on success. Pure: adapters and the permit
//! callback come in as arguments, and the router makes no HTTP or database calls.

use std::collections::{HashMap, HashSet};

use deepref_domain::{ProviderKind, Reference, normalize_doi};
use serde::Serialize;
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::acquisition::{
    FetchedWork, ProviderFetchError, ProviderPermit, ReferenceGap, WorkProvider,
};

/// What one attempt produced. Serialized into `raw.deepref_provider_attempts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AttemptOutcome {
    Answered,
    /// The metadata answered, but the daily budget ran out before every reference resolved.
    BudgetExhausted,
    NotFound,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderAttempt {
    pub provider: ProviderKind,
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
    pub attempts: Vec<ProviderAttempt>,
    pub result: Result<FetchedWork, RouteError>,
    /// Set when the answer came back without every reference it listed.
    pub lost_edges: Option<LostEdges>,
}

/// A budget stop that cost an answer some references, and what the router did about it.
/// The application layer has no logging, so the worker reports this record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LostEdges {
    pub provider: ProviderKind,
    /// `expected - resolved` at the stop.
    pub count: usize,
    pub reset_secs: Option<u64>,
    pub fill: LostEdgeFill,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LostEdgeFill {
    /// Crossref's reference list was merged, adding `added` references the answer lacked.
    Crossref { added: usize },
    /// Crossref is not in the run's chain, so nothing could fill the edges.
    CrossrefNotEnabled,
    /// Crossref's reference list could not be read.
    CrossrefFailed { message: String },
}

impl LostEdges {
    fn crossref_contributed(&self) -> bool {
        matches!(self.fill, LostEdgeFill::Crossref { added } if added > 0)
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

    /// Runs `doi` through the chain effective for `stored`. Takes one permit before
    /// each attempt. A permit that cannot be taken is a retryable attempt that never
    /// reaches its provider.
    ///
    /// NotFound moves on. Rejected ends the chain as not found with its reason.
    /// Retryable moves on, and the fetch is retryable when nothing answers. Fatal
    /// stops the chain.
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
        let chain_kinds: Vec<ProviderKind> = chain.iter().map(|(kind, _)| *kind).collect();
        let mut attempts = Vec::with_capacity(chain.len());
        // Providers whose adapter ran. A permit that was refused never reached one.
        let mut tried = Vec::with_capacity(chain.len());
        let mut last_retryable = None;
        for (kind, adapter) in chain {
            let result = match permit.acquire(kind).await {
                Ok(()) => {
                    let result = adapter.fetch_work_after(doi, &tried, permit).await;
                    tried.push(kind);
                    result
                }
                Err(error) => Err(ProviderFetchError::Retryable {
                    provider: kind,
                    message: error.to_string(),
                }),
            };
            match result {
                Ok(fetched) => {
                    let gap = fetched.reference_gap;
                    let outcome = if gap.is_some() {
                        AttemptOutcome::BudgetExhausted
                    } else {
                        AttemptOutcome::Answered
                    };
                    attempts.push(attempt(kind, outcome));
                    let mut fetched = fetched;
                    let mut contributions = vec![kind];
                    let mut lost_edges = None;
                    if let Some(gap) = gap {
                        let lost = self
                            .fill_lost_edges(
                                &chain_kinds,
                                kind,
                                doi,
                                gap,
                                &mut fetched.work.references,
                                permit,
                            )
                            .await;
                        if lost.crossref_contributed() {
                            contributions.push(ProviderKind::Crossref);
                        }
                        lost_edges = Some(lost);
                    }
                    let fetched = stamp_provenance(fetched, &attempts, &contributions);
                    return RouteOutput {
                        attempts,
                        result: Ok(fetched),
                        lost_edges,
                    };
                }
                Err(ProviderFetchError::NotFound { .. }) => {
                    attempts.push(attempt(kind, AttemptOutcome::NotFound));
                }
                Err(ProviderFetchError::Rejected { reason, .. }) => {
                    attempts.push(attempt(
                        kind,
                        AttemptOutcome::Rejected {
                            reason: reason.clone(),
                        },
                    ));
                    return RouteOutput {
                        attempts,
                        result: Err(RouteError::NotFound {
                            reason: Some(reason),
                        }),
                        lost_edges: None,
                    };
                }
                Err(ProviderFetchError::Retryable { message, .. }) => {
                    attempts.push(attempt(
                        kind,
                        AttemptOutcome::Retryable {
                            message: message.clone(),
                        },
                    ));
                    last_retryable = Some(message);
                }
                Err(ProviderFetchError::Fatal { message, .. }) => {
                    attempts.push(attempt(
                        kind,
                        AttemptOutcome::Fatal {
                            message: message.clone(),
                        },
                    ));
                    return RouteOutput {
                        attempts,
                        result: Err(RouteError::Fatal { message }),
                        lost_edges: None,
                    };
                }
            }
        }
        let error = match last_retryable {
            Some(message) => RouteError::Retryable { message },
            None => RouteError::NotFound { reason: None },
        };
        RouteOutput {
            attempts,
            result: Err(error),
            lost_edges: None,
        }
    }

    /// Fills the references that a budget stop cost the answer, from Crossref's reference
    /// list when Crossref is in the chain. The answer is never dropped: a Crossref that is
    /// missing or fails leaves the partial references in place, and the report says so.
    async fn fill_lost_edges(
        &self,
        chain: &[ProviderKind],
        answering: ProviderKind,
        doi: &str,
        gap: ReferenceGap,
        references: &mut Vec<Reference>,
        permit: &dyn ProviderPermit,
    ) -> LostEdges {
        let ReferenceGap::BudgetExhausted {
            resolved,
            expected,
            reset_secs,
        } = gap;
        let crossref = self.adapters.get(&ProviderKind::Crossref).filter(|_| {
            answering != ProviderKind::Crossref && chain.contains(&ProviderKind::Crossref)
        });
        let fill = match crossref {
            None => LostEdgeFill::CrossrefNotEnabled,
            Some(crossref) => match crossref.reference_list(doi, permit).await {
                Ok(list) => LostEdgeFill::Crossref {
                    added: merge_references(references, list),
                },
                Err(error) => LostEdgeFill::CrossrefFailed {
                    message: error.to_string(),
                },
            },
        };
        LostEdges {
            provider: answering,
            count: expected.saturating_sub(resolved),
            reset_secs,
            fill,
        }
    }
}

/// Appends the references in `list` whose normalized DOI is new, and returns how many were
/// added. Entries without a DOI cannot become child jobs, so they stay out.
fn merge_references(references: &mut Vec<Reference>, list: Vec<Reference>) -> usize {
    let mut known: HashSet<String> = references
        .iter()
        .filter_map(|reference| reference.doi.as_deref())
        .filter_map(|doi| normalize_doi(doi).ok())
        .collect();
    let mut added = 0;
    for mut reference in list {
        let Some(doi) = reference
            .doi
            .as_deref()
            .and_then(|doi| normalize_doi(doi).ok())
        else {
            continue;
        };
        if known.insert(doi.clone()) {
            reference.doi = Some(doi);
            references.push(reference);
            added += 1;
        }
    }
    added
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

fn attempt(provider: ProviderKind, outcome: AttemptOutcome) -> ProviderAttempt {
    ProviderAttempt { provider, outcome }
}

/// Records the answering provider, its attempts and the reference sources on the work and in
/// its raw payload. `contributions` lists the reference sources in the order they contributed.
fn stamp_provenance(
    mut fetched: FetchedWork,
    attempts: &[ProviderAttempt],
    contributions: &[ProviderKind],
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
    use crate::acquisition::{PermitError, ProviderFuture};

    const DOI: &str = "10.1000/example";

    /// Adapter calls in the order they ran, shared by every fake in one router.
    type Calls = Arc<Mutex<Vec<ProviderKind>>>;
    /// Each lookup that ran, with the providers it was told had already tried the DOI.
    type Tried = Arc<Mutex<Vec<(ProviderKind, Vec<ProviderKind>)>>>;

    struct FakeProvider {
        kind: ProviderKind,
        answer: Result<WorkWithReferences, ProviderFetchError>,
        gap: Option<ReferenceGap>,
        list: Result<Vec<Reference>, ProviderFetchError>,
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
            _permit: &'a dyn ProviderPermit,
        ) -> ProviderFuture<'a, FetchedWork, ProviderFetchError> {
            Box::pin(async move {
                self.calls.lock().expect("call log").push(self.kind);
                self.answer.clone().map(|work| FetchedWork {
                    work,
                    provider: self.kind,
                    reference_gap: self.gap,
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
                    .acquire(self.kind)
                    .await
                    .map_err(|error| ProviderFetchError::Retryable {
                        provider: self.kind,
                        message: error.to_string(),
                    })?;
                self.list.clone()
            })
        }
    }

    #[derive(Default)]
    struct FakePermit {
        requested: Mutex<Vec<ProviderKind>>,
        unavailable: bool,
    }

    impl ProviderPermit for FakePermit {
        fn acquire<'a>(&'a self, provider: ProviderKind) -> ProviderFuture<'a, (), PermitError> {
            Box::pin(async move {
                self.requested.lock().expect("permit log").push(provider);
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
            gap: None,
            list: Ok(Vec::new()),
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

    fn budget_gap(resolved: usize, expected: usize) -> Option<ReferenceGap> {
        Some(ReferenceGap::BudgetExhausted {
            resolved,
            expected,
            reset_secs: Some(3_600),
        })
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

    fn outcomes(output: &RouteOutput) -> Vec<(ProviderKind, AttemptOutcome)> {
        output
            .attempts
            .iter()
            .map(|attempt| (attempt.provider, attempt.outcome.clone()))
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
                (ProviderKind::OpenAlex, AttemptOutcome::NotFound),
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
                (ProviderKind::OpenAlex, AttemptOutcome::NotFound),
                (ProviderKind::Crossref, AttemptOutcome::NotFound),
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
        assert_eq!(fetched.work.work.citation_provider, "datacite");
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
    fn a_fatal_failure_stops_the_chain() {
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
                fake(ProviderKind::DataCite, answers(), &calls),
            ],
        );

        let output = run(&router, &[], &FakePermit::default());

        assert_eq!(
            output.result.as_ref().err().cloned(),
            Some(RouteError::Fatal {
                message: "bad request".to_owned(),
            })
        );
        assert_eq!(
            call_log(&calls),
            vec![ProviderKind::OpenAlex, ProviderKind::Crossref]
        );
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
            permit.requested.lock().expect("permit log").clone(),
            vec![
                ProviderKind::OpenAlex,
                ProviderKind::Crossref,
                ProviderKind::DataCite,
            ]
        );
    }

    #[test]
    fn a_permit_that_cannot_be_taken_fails_the_attempt_before_the_provider_runs() {
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

        assert!(call_log(&calls).is_empty());
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
                fake(ProviderKind::Crossref, answers(), &calls),
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
                { "provider": "openalex", "outcome": "not_found" },
                { "provider": "crossref", "outcome": "answered" },
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
    fn a_budget_gap_fills_the_lost_edges_from_crossref_and_dedupes_by_doi() {
        let calls = Calls::default();
        let permit = FakePermit::default();
        let tried = Tried::default();
        let openalex = FakeProvider {
            gap: budget_gap(2, 4),
            ..provider(
                ProviderKind::OpenAlex,
                Ok(work_citing(vec![
                    reference(Some("10.1000/a")),
                    reference(Some("10.1000/b")),
                ])),
                &calls,
                &tried,
            )
        };
        let crossref = FakeProvider {
            list: Ok(vec![
                reference(Some("10.1000/B")),
                reference(Some("10.1000/c")),
                reference(None),
            ]),
            ..provider(ProviderKind::Crossref, answers(), &calls, &tried)
        };
        let router = router(
            &[ProviderKind::OpenAlex, ProviderKind::Crossref],
            vec![boxed(openalex), boxed(crossref)],
        );

        let output = run(&router, &[], &permit);

        let fetched = output.result.as_ref().expect("the OpenAlex answer is kept");
        assert_eq!(fetched.work.work.metadata_provider, "openalex");
        assert_eq!(fetched.work.work.citation_provider, "openalex,crossref");
        assert_eq!(
            output.lost_edges,
            Some(LostEdges {
                provider: ProviderKind::OpenAlex,
                count: 2,
                reset_secs: Some(3_600),
                fill: LostEdgeFill::Crossref { added: 1 },
            })
        );
        assert_eq!(
            reference_dois(fetched),
            vec!["10.1000/a", "10.1000/b", "10.1000/c"]
        );
        assert_eq!(
            fetched.work.raw["deepref_provider_attempts"],
            json!([{ "provider": "openalex", "outcome": "budget_exhausted" }])
        );
        assert_eq!(
            fetched.work.raw["deepref_citation_sources"],
            json!(["openalex", "crossref"])
        );
        assert_eq!(
            outcomes(&output),
            vec![(ProviderKind::OpenAlex, AttemptOutcome::BudgetExhausted)]
        );
        assert_eq!(call_log(&calls), vec![ProviderKind::OpenAlex]);
        assert_eq!(
            permit.requested.lock().expect("permit log").clone(),
            vec![ProviderKind::OpenAlex, ProviderKind::Crossref]
        );
    }

    #[test]
    fn a_budget_gap_with_crossref_outside_the_chain_keeps_the_partial_references() {
        let calls = Calls::default();
        let permit = FakePermit::default();
        let tried = Tried::default();
        let openalex = FakeProvider {
            gap: budget_gap(1, 3),
            ..provider(
                ProviderKind::OpenAlex,
                Ok(work_citing(vec![reference(Some("10.1000/a"))])),
                &calls,
                &tried,
            )
        };
        let crossref = FakeProvider {
            list: Ok(vec![reference(Some("10.1000/b"))]),
            ..provider(ProviderKind::Crossref, answers(), &calls, &tried)
        };
        let router = router(
            &[ProviderKind::OpenAlex],
            vec![boxed(openalex), boxed(crossref)],
        );

        let output = run(&router, &[], &permit);

        let fetched = output.result.as_ref().expect("the OpenAlex answer is kept");
        assert_eq!(fetched.work.work.citation_provider, "openalex");
        assert_eq!(reference_dois(fetched), vec!["10.1000/a"]);
        assert_eq!(
            outcomes(&output),
            vec![(ProviderKind::OpenAlex, AttemptOutcome::BudgetExhausted)]
        );
        assert_eq!(call_log(&calls), vec![ProviderKind::OpenAlex]);
        assert_eq!(
            output.lost_edges,
            Some(LostEdges {
                provider: ProviderKind::OpenAlex,
                count: 2,
                reset_secs: Some(3_600),
                fill: LostEdgeFill::CrossrefNotEnabled,
            })
        );
        assert_eq!(
            permit.requested.lock().expect("permit log").clone(),
            vec![ProviderKind::OpenAlex]
        );
    }

    #[test]
    fn a_failing_crossref_reference_list_keeps_the_answer_and_its_partial_references() {
        let calls = Calls::default();
        let permit = FakePermit::default();
        let tried = Tried::default();
        let openalex = FakeProvider {
            gap: budget_gap(1, 2),
            ..provider(
                ProviderKind::OpenAlex,
                Ok(work_citing(vec![reference(Some("10.1000/a"))])),
                &calls,
                &tried,
            )
        };
        let crossref = FakeProvider {
            list: Err(ProviderFetchError::Retryable {
                provider: ProviderKind::Crossref,
                message: "upstream timed out".to_owned(),
            }),
            ..provider(ProviderKind::Crossref, answers(), &calls, &tried)
        };
        let router = router(
            &[ProviderKind::OpenAlex, ProviderKind::Crossref],
            vec![boxed(openalex), boxed(crossref)],
        );

        let output = run(&router, &[], &permit);

        let fetched = output.result.as_ref().expect("the OpenAlex answer is kept");
        assert_eq!(fetched.work.work.metadata_provider, "openalex");
        assert_eq!(fetched.work.work.citation_provider, "openalex");
        assert_eq!(
            output.lost_edges,
            Some(LostEdges {
                provider: ProviderKind::OpenAlex,
                count: 1,
                reset_secs: Some(3_600),
                fill: LostEdgeFill::CrossrefFailed {
                    message: "crossref is unavailable for now: upstream timed out".to_owned(),
                },
            })
        );
        assert_eq!(reference_dois(fetched), vec!["10.1000/a"]);
        assert_eq!(
            outcomes(&output),
            vec![(ProviderKind::OpenAlex, AttemptOutcome::BudgetExhausted)]
        );
        assert_eq!(
            permit.requested.lock().expect("permit log").clone(),
            vec![ProviderKind::OpenAlex, ProviderKind::Crossref]
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
}
