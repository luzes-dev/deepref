# Metadata provider routing handoff

> **Status:** designed 2026-10-17, revised after review 2026-10-18. Not implemented.
> **Base:** builds on #250 (`DEEPREF_CONTACT_EMAIL`, merged), which introduced `IntegrationsConfig`.
> **Scope:** DOI metadata fetching, reference edges, provider configuration, provenance, persistence touchpoints, and the web strings that name providers. Publication alerts and PMID imports keep their current providers.
> **Audience:** the implementing agent. Confirm section 10 with the user before Phase 1a. Phase 0 is behavior-neutral and can start now.

## 1. Objective

Replace the single Crossref client with a configurable chain of metadata providers. OpenAlex answers first. Crossref falls back when OpenAlex does not know the DOI or cannot answer. DataCite catches DOIs that neither holds, mostly datasets, software and repository records.

Three invariants survive the change. A work row records which provider answered it, DOI traversal keeps filling child DOIs from reference lists, and a missing contact e-mail disables the DOI lane without stopping any process from booting.

## 2. Current code anchors

| Area | Location | What it does today |
|---|---|---|
| Crossref client | `crates/integrations/src/crossref.rs` | `fetch_work_with_references`, `search_crossref`, error classification, `MetadataProvider`/`CitationProvider` impls with no consumers |
| Crossref wire mapping | `crates/integrations/src/crossref/wire.rs` | Crossref JSON to `WorkWithReferences`, references with optional DOI |
| Abstract enrichment | `crates/integrations/src/crossref/abstracts.rs` | Europe PMC then OpenAlex, best-effort; already talks to OpenAlex |
| DOI acquisition worker | `services/worker/src/processor.rs` | Per-DOI job: `handle_work_fetch` claims the DOI, takes one permit, fetches with `CrossrefClient`, persists work plus citations plus child jobs |
| Provider retries | `crates/integrations/src/crossref.rs` | `max_attempts` loop inside the client, fed from `settings.retry_attempts`; the worker adds job-level retries on top |
| Persistence | `services/worker/src/store.rs` | Work upsert and DOI lane labels; `persist_citation` writes `citations.source='crossref-reference'` and inherits the same default on `fetched_citation_facts` |
| Fetch cache | `services/worker/src/store.rs` | `is_cached` and `attach_cached` reuse any fetched work by DOI across runs, projects and providers; the run chain never enters the decision |
| Run lock | `services/worker/src/store.rs` | `mark_fetching` locks the run and runs once before the fetch; `lock_run` is also called by four other store functions |
| Event payload | `crates/domain/src/events/work.rs` | `WorkFetchRequested` carries project, acquisition, DOI and depths; no provider fields |
| Workflow import node | `services/worker/src/workflows/nodes.rs` | `import_identifiers` builds a `CrossrefClient` directly for DOIs; it has no run row |
| Alerts | `services/worker/src/workflows.rs` | `search_crossref` plus PubMed search on a date window |
| Rate limiting | `services/worker/src/limiter.rs`, `deepref_app::reserve_provider_permit` | Per-provider permit keyed by a string; any provider name works |
| Acquisition API | `crates/http-api/src/routes/acquisitions.rs` | `create_acquisition` defaults both providers to the hardcoded string `crossref` and rejects anything else; `parse_stored_doi_config` requires exactly `crossref`; `list_acquisition_items` sends `acquisition_runs.source='crossref'` to the DOI query and every other value to the PMID import query; `refresh_acquisition` requires the same literal |
| Settings API | `crates/http-api/src/routes/settings.rs` | Returns `metadata_provider`/`citation_provider`; the UI does not show them |
| Web call sites | `apps/web/src/lib/features/projects/components/ImportDoiForm.svelte`, `apps/web/src/lib/features/projects/imports.ts`, `apps/web/tests/settings-shallow.e2e.ts`, `apps/web/tests/visual/api-fixture.ts`, `apps/web/src/routes/page.svelte.e2e.ts`, `apps/web/src/tests/mocks/handlers.ts` | Send hardcoded `crossref`; label says "Not found in Crossref" |
| Config | `crates/config/src/integrations.rs` | `IntegrationsConfig { abstract_enrichment, contact_email }`, validated at boot |
| Domain | `crates/domain/src/work.rs`, `crates/domain/src/acquisition.rs` | `Work.metadata_provider`/`citation_provider` as text; `AcquisitionSource` has `OpenAlex` and `Other(String)`, so a typed `DataCite` variant is a nicety, not a requirement |
| Unused traits | `crates/application/src/acquisition.rs` | `MetadataProvider`, `CitationProvider`, `SearchProvider`; only Crossref implements the first two and nothing calls them |
| Telemetry | `crates/telemetry/src` | Traces only; no metric counters, gauges or histograms exist |

## 3. Provider facts

Provider documentation and live API observations checked on 2026-10-17; the source links below distinguish documented contracts from observed behavior.

**OpenAlex access model**

| Operation | Cost | Limit |
|---|---|---|
| Get one work by DOI | Free, unlimited | 10 req/s without a key, 30 with a free key, 50 on annual plans |
| List plus filter, includes batch `ids.openalex:` and `cites:` filters | US$ 0.0001 per call | 100 OR values per filter, `per_page` max 100 |
| Search | US$ 0.001 per call | same per-second tiers |

Daily budget: US$ 0.10 shared per IP without a key, US$ 1 with a free key. A 429 covers both per-second limiting and budget exhaustion. Read `X-RateLimit-Remaining` to tell them apart: zero means the daily budget is gone until the reset reported in `X-RateLimit-Reset`, seconds until midnight UTC; above zero means slow down. Error responses are free. Cursor paging needs a key.

Keys have three states, confirmed against the live API on 2026-10-17: a valid key, a pending key that returns 403 `key_not_activated` until its owner clicks the activation link, and an invalid or revoked key that returns 401 `Invalid or missing API key` on every endpoint, including the usage one.

**OpenAlex corpus**

Core is the default for list queries and already includes Crossref, PubMed, DataCite and repository records: 320M+ works. The expansion adds about 190M dataset and repository records without a core match, reachable with `corpus=all`. New works always land in core. The [corpus documentation](https://help.openalex.org/data/works/corpus/) describes a core default, but a [live singleton lookup of `10.5281/zenodo.15521699`](https://api.openalex.org/works/https://doi.org/10.5281/zenodo.15521699?select=id,doi,is_xpac) returned `is_xpac: true` without a corpus parameter. Do not assume singleton misses identify expansion records; enforce the configured corpus on returned records as specified in 5.3.

**OpenAlex fields worth using**

`referenced_works` holds references resolved to OpenAlex IDs only. [`referenced_works_count` is its length](https://help.openalex.org/data/works/attributes/#referenced_works_count), not the original bibliography size. Both omit references OpenAlex never matched; see [citation limitations](https://help.openalex.org/data/works/citations/). Comparing these values cannot identify those lost edges. `ids` carries PMID, PMCID and MAG IDs. `best_oa_location` and `locations` carry PDF URLs, licenses and versions. `is_retracted` and `type == "paratext"` flag quality; `is_paratext` is deprecated. `authorships` carries institutions, ORCID and corresponding flags. Topics, keywords, `funders`, `awards`, `counts_by_year` and `related_works` are extras for raw. The removed `grants` property must not appear in `select=`; use [`funders` and `awards`](https://help.openalex.org/data/works/attributes/#funders).

**DataCite**

Singleton by DOI at `api.datacite.org/dois/{id}`, JSON:API shape, with `relatedIdentifiers`, `descriptions`, `creators` with ORCID, `types.resourceTypeGeneral`, `rightsList` and an optional `enriched=true` layer. Identified via `User-Agent` with `mailto:`, the limit is 1000 requests per 5 minutes per IP, about 3.3 per second. 429 means wait and retry.

Relation direction matters. `References` and `Cites` point from this record to the target and become outbound reference edges. `IsReferencedBy` and `IsCitedBy` point the other way, another work cites this one, and never become `Reference` entries.

**Crossref**

Authoritative for Crossref DOIs and first to see a new registration. Reference lists include entries without a resolved DOI, and the lookup is free. A multi-DOI filter (`filter=doi:a,doi:b`) works for lookups, but there is no batch response for full metadata. Rate limits were revised in December 2025 and the existing per-provider permit stays in charge.

## 4. Provider strengths and routing priority

| Task | Winner | Why |
|---|---|---|
| Article metadata by DOI | OpenAlex | Free singleton get, abstract, topics, institutions, OA and retraction in one record |
| Authoritative record and freshness | Crossref | The DOI registration is the source; indexing comes later |
| Outgoing references | OpenAlex plus Crossref | OpenAlex gives resolved edges, Crossref gives raw entries OpenAlex could not resolve, and free reference lists |
| Incoming citations | OpenAlex | `filter=cites:W...`, no equivalent elsewhere; DataCite incoming relations feed the same idea in Phase 5 |
| Datasets and software metadata | DataCite | Registration authority for those DOIs, with typed outbound relations |
| Abstract | OpenAlex | Wide coverage from the inverted index; Europe PMC stays as backup on Crossref answers |
| Open-access PDF | OpenAlex | Aggregates Unpaywall data; Unpaywall becomes the fallback |
| Alerts and search | Crossref plus PubMed | Registration precedes indexing, and search costs ten times a list call |
| Enrichment fields | OpenAlex | Topics, institutions, funders, awards, retraction and citation counts are first-class |

Priority by operation:

- Metadata by DOI: `openalex` then `crossref` then `datacite`.
- References: OpenAlex `referenced_works` resolved in batches, always unioned with Crossref reference lists on an OpenAlex answer when Crossref is deployment-enabled, regardless of counts or budget state. DataCite answers use their `References`/`Cites` relations.
- Alerts: unchanged.

## 5. Target design

### 5.1 Configuration

Extend `IntegrationsConfig` in `crates/config/src/integrations.rs`:

- `metadata_providers: Vec<ProviderKind>` from `DEEPREF_METADATA_PROVIDERS`.
- `openalex_api_key: Option<String>` from `DEEPREF_OPENALEX_API_KEY`.
- `openalex_corpus: Core | All` from `DEEPREF_OPENALEX_CORPUS`, default `core`.

There is no citation chain environment variable and no citation chain in the API. References follow the union rules in 5.3 and 5.4, not a provider walk. There is no run-level citation field either: a run spans many works, and the contributing sources are recorded per work in `Work.citation_provider`, for example `openalex` or `openalex,crossref`.

The default moves in stages so every phase ships behavior the adapters support:

| Phase | Default |
|---|---|
| 0 | `crossref`, no behavior change |
| 1a | `crossref`, routed through the new port |
| 1b | `openalex,crossref` |
| 2 | `openalex,crossref,datacite` |

The configured chain is also the deployment allowlist for metadata routing and the Crossref reference helper, not a strict outbound-provider allowlist:

- A job's effective chain is the chain stored on its run, filtered to enabled providers, preserving the stored order.
- Enabled means named in the deployment chain and implemented by an adapter.
- If the filtered chain is empty, the job uses the deployment chain. Config validation keeps that chain non-empty.
- A stored chain that names a provider later removed from the allowlist is filtered out, not an error. This makes `DEEPREF_METADATA_PROVIDERS=crossref` a rollback of metadata routing for jobs started after the configuration is loaded, including queued jobs from existing runs. Already executing HTTP requests and globally cached works are not changed.
- The existing Crossref abstract-enrichment path may still call OpenAlex in a Crossref-only deployment. That preserves current behavior; blocking all OpenAlex egress is not promised by this rollback. The enrichment rules and permits are specified in 5.2.

Config errors still refuse to start: unknown name, a name with no adapter yet, or an empty configured list. A missing contact e-mail is not a config error; see 5.2.

Rules: trim entries, drop duplicates keeping first position, reject an empty or unknown list with `ConfigError::Invalid`, and reject a name that has no adapter yet with a clear message. A missing key is valid. Log the effective chain once at API and worker boot.

Rate values: OpenAlex 25 per second with a key and 8 per second after a keyless downgrade, DataCite 3 per second, constants in `crates/integrations`. Crossref keeps `settings.rate_limit_per_second`.

`ProviderKind` lives in `crates/domain/src/provider.rs` with `FromStr`, `as_str` and snake_case serde.

The user obtains the OpenAlex key from the OpenAlex settings page. The key never lands in `.env.example`, fixtures, or commits.

### 5.2 Provider layer

The port lives in `crates/application`, per ADR 0002, and `crates/integrations` implements it:

```rust
pub type ProviderFuture<'a, T> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<T, ProviderFetchError>> + Send + 'a>,
>;

pub trait WorkProvider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    fn fetch_work_with_references<'a>(&'a self, doi: &'a str)
        -> ProviderFuture<'a, FetchedWork>;
}

pub struct FetchedWork {
    pub work: WorkWithReferences,
    pub provider: ProviderKind,
}

pub enum ProviderFetchError {
    NotFound { provider: ProviderKind, doi: String },
    Rejected { provider: ProviderKind, reason: String },
    Retryable { provider: ProviderKind, message: String },
    Fatal { provider: ProviderKind, message: String },
}
```

Delete `MetadataProvider`, `CitationProvider` and `SearchProvider` and their Crossref impls and re-exports in the same phase. `RawRecord` stays for import parsers; `ImportParser::parse` is synchronous and does not use `ProviderFuture`. Replace the existing future alias's fixed `ProviderError` with `ProviderFetchError` as shown above. Keep `ProviderError` only for real remaining consumers, such as the Crossref constructor, and remove obsolete variants.

Attempt semantics: one attempt is one adapter invocation for a purpose: metadata, reference resolution, reference union, or abstract enrichment. It can issue multiple HTTP requests, including internal retries; record its request count and final outcome. Metadata and reference calls use the current policy fed by `settings.retry_attempts`; abstract enrichment retains its existing best-effort retry policy. The router adds no retry loop of its own: metadata routing moves to the next provider on `NotFound` or exhausted `Retryable`. Job-level retries stay as they are. No third layer.

Permits: inject an asynchronous permit callback when constructing the router, adapters and abstract enricher. Acquire immediately before every outbound HTTP request, including each internal retry, reference batch, Crossref helper call, boot probe and Europe PMC/OpenAlex enrichment request. Remove the worker's one-per-fetch permit; it cannot account for internal requests. Use the actual outbound provider's key, and resolve OpenAlex's keyed/keyless rate at request time so downgrade also lowers the permit rate. Permit acquisition failures propagate as retryable infrastructure failures, not provider not-found answers.

Metadata flow rules: `NotFound` moves to the next provider. `Rejected` ends the chain for that DOI on the spot and maps to `IngestionItemStatus::NotFound` with the reason in `last_error`; it is not a job failure and no later provider is asked. `Retryable` exhausts the adapter's policy, then moves on. `Fatal` is a provider-level failure that retrying cannot fix (other 4xx, generic 403, keyless 401, malformed response). Record it in the attempt history and move to the next provider, so one broken provider cannot fail every DOI. If every provider returned `NotFound`, the fetch is not found. If a provider failed retryably and none succeeded, the fetch is retryable so the job comes back later. If none succeeded and the failures were only fatal, the fetch fails terminally with the attempt summary in `last_error`. Once valid metadata has been selected, reference failures follow the table below; they do not silently select a different metadata provider.

Reference outcomes are separate from metadata outcomes:

| After metadata succeeds | Reference handling | Router result and cache |
|---|---|---|
| All required reference requests finish, including Crossref union when enabled | Normalize and deduplicate DOI edges; retain Crossref entries without DOIs | Success, reference status `complete` |
| A successful batch omits requested IDs, or targets have null/invalid DOIs | Keep valid DOI edges, record missing/DOI-less target counts, and still run Crossref union when enabled | Success, reference status `degraded` |
| OpenAlex's paid reference budget is exhausted or already paused | Stop paid calls until reset; retain earlier batches and run Crossref union when enabled | Success, reference status `degraded`, provided the helper does not fail transiently |
| A reference batch or enabled Crossref helper exhausts transient retries | Retain attempt history, but do not finalize/cache the metadata or partial references | Retryable job failure; do not advance the metadata chain |
| Crossref helper returns 404 | Record not found; contribute no Crossref references | Success if other required requests finished; preserve any existing degradation |
| An OpenAlex reference request returns a fatal error, including a malformed list response | Retain attempt history; do not finalize/cache partial results | Fatal request failure; the metadata chain does not advance |
| The enabled Crossref helper returns a fatal error, including malformed response data | Record the attempt with reason `helper_fatal`; keep the OpenAlex references | Success, reference status `degraded` with reason `helper_fatal`; cached like other degradation |

`complete` means the required fetch plan completed, not that a provider captured the entire bibliography. A genuinely empty reference list is not degradation. Best-effort abstract-enrichment failures never change a successful metadata/reference outcome.

The contact e-mail is required for the whole DOI lane, not per provider. Every adapter sends it as `mailto`, and `persist_doi_acquisition` already refuses DOI acquisitions, create and refresh, when it is empty. Without it, the API and worker boot, warn once, and leave the DOI lane unavailable: the API keeps its current `DEEPREF_CONTACT_EMAIL` error, and worker jobs and the import node fail with the current message, exactly as they do today. No provider is dropped individually for a missing e-mail. Phase 0 stays behavior-neutral this way.

An OpenAlex key that fails is not a reason to disable OpenAlex: the adapter downgrades to keyless per 5.3.

Provenance: `Work.metadata_provider` names the provider that answered the metadata fetch. `Work.citation_provider` records providers that contributed retained resolved or unresolved references, in contribution order and deduplicated, for example `openalex` or `openalex,crossref`; it is empty when no provider contributed references. Raw carries `deepref_metadata_source`, `deepref_provider_attempts`, `deepref_citation_sources`, and `deepref_reference_status` (`complete` or `degraded`) with degradation reasons and missing/DOI-less target counts. Attempt entries distinguish provider, purpose, outcome and HTTP request count, so metadata fallback and reference fallback are not confused.

The router returns structured attempt history on both success and failure. Its failure result contains the final `ProviderFetchError` and the attempt entries even when no work/raw payload exists. The worker emits those entries as tracing events and includes a bounded, credential-free summary in the existing `last_error`; terminal persistence takes the last attempted provider from structured data, never by parsing an error string. Successful results serialize the same history into raw. Do not persist partial work merely to retain failure diagnostics.

Abstract enrichment: pass per-fetch context indicating whether the metadata chain already tried OpenAlex. On a Crossref fallback after such an attempt, disable OpenAlex inside the enricher and still try Europe PMC. A Crossref-only chain retains today's Europe PMC then OpenAlex enrichment. Both use per-request permits, and OpenAlex enrichment shares the adapter's key state and rate policy rather than creating an independent unthrottled path. Scope this routing-specific behavior to DOI fetching; alerts keep their existing provider choices.

### 5.3 OpenAlex adapter

Base `https://api.openalex.org`, identification via `User-Agent` and `mailto` from `contact_email`, `select=` per fetch shape, base URL injectable for tests.

Mapping to `WorkWithReferences`:

- `display_name` to title, `abstract_inverted_index` reconstructed to plain text through the existing `clean_abstract`.
- `publication_year` and `publication_date` to year fields, `type` to work type.
- `primary_location.source.display_name` to container title, `host_organization_name` to publisher.
- `cited_by_count` to total citations, `referenced_works_count` to the provider-reported references count; do not overwrite that count with the union's size or treat it as the original bibliography size.
- `best_oa_location` and `locations` into `raw.deepref_oa_urls`.
- Preserve the selected OpenAlex response in `raw`, including `is_retracted`, `type`, `is_xpac`, `ids`, `topics`, `authorships`, `funders` and `awards`. Raw means the response to the documented `select=`, not an unrequested full object. Retain these fields from Phase 1b so cache hits do not depend on later refetches.

Status table:

| Response | Meaning | Handling |
|---|---|---|
| 200 | Work found | Run identity, corpus eligibility and paratext checks below before resolving references |
| 301 | Entity merged, followed by the HTTP client | The identity check decides |
| 404 | DOI unknown | `NotFound`, next provider |
| 429 on a singleton | Per-second limit; singleton calls are free | `Retryable` with backoff |
| 429 on a list or search, `X-RateLimit-Remaining` above zero | Per-second limit | `Retryable` with backoff |
| 429 on a paid call with `X-RateLimit-Remaining` at zero | Daily budget exhausted | Budget path below, not a retry |
| 401, or 403 with `key_not_activated`, on a keyed request | Key invalid, revoked, or pending | Warn once, mark the key unusable in this process, retry the request keyless with a new permit, continue keyless until restart |
| Other 403, or 401 on an already keyless request | Authorization failure, not evidence of a pending key | `Fatal`; do not loop or downgrade unrelated requests |
| Other 4xx or malformed successful response | Invalid request or response contract | `Fatal` |
| 5xx or network failure | Temporary | `Retryable` |

Boot probe: when a key and contact e-mail are present, take a permit and call `GET /rate-limit` once with the key, expected free. 200 means keyed mode. 401, or 403 `key_not_activated`, means warn once and continue keyless, with the lower rate. Other HTTP failures or a network failure retain the configured key and log a warning; runtime handling applies when a fetch is attempted. A key activated after boot takes effect on the next restart. Use the same authentication classification at boot and runtime: [pending-key documentation](https://help.openalex.org/api/authentication/#getting-a-key-for-your-user) identifies `key_not_activated`, while [general 403 responses](https://help.openalex.org/api/errors/#http-status-codes) do not imply a bad key.

Identity check: compare the DOI OpenAlex returns with the requested one through `normalize_doi`. A mismatch, including a null DOI, makes OpenAlex answer not found for that request, logged with both DOIs, and the chain falls through. A sibling version must never be filed under the requested DOI.

Paratext: after identity and corpus eligibility checks, `type == "paratext"` returns `Rejected` with reason `paratext`, maps to not found, and never reaches another provider or reference resolution. A Crossref fall-through would not enforce this exclusion. This policy ships in Phase 1b, before any OpenAlex answer enters the global cache.

References: resolve `referenced_works` with `ceil(n / 100)` list calls per work, `filter=ids.openalex:W1|...|W100&select=id,doi&per_page=100`. A typical article with 30 resolved references is one call; a review with 250 is three. Only targets with a valid normalized DOI become `Reference { doi: Some(..) }`. Record omitted IDs and DOI-less targets as counts in raw, not fabricated unresolved bibliographic entries.

Crossref union: on every accepted OpenAlex metadata answer, the router also asks the Crossref reference-list helper when Crossref is deployment-enabled, even if the run's metadata chain contains only OpenAlex. This is one free lookup with its own permits/retry policy, without abstract enrichment or a metadata-provenance change. Merge in contribution order: OpenAlex references first, then Crossref references. Deduplicate by normalized DOI, retain Crossref bibliographic fields for a duplicate, and retain all Crossref entries without a DOI through the existing unresolved-reference path. Never gate this helper on `referenced_works_count`; it cannot detect unmatched bibliography entries. The helper ships in Phase 1a and the unconditional union ships in Phase 1b.

Budget exhaustion: when a paid reference-resolution call returns 429 with `X-RateLimit-Remaining` at zero, stop issuing paid list calls in this process until the reset time derived from `X-RateLimit-Reset`. Record `budget_exhausted` and the reset time in the current fetch's attempt history, including fetches that skip calls because the budget is already paused. Metadata still comes from OpenAlex, since singleton fetches are free. Keep any resolved batches, still perform the Crossref union when enabled, and mark reference status `degraded`. Without Crossref, zero or partial references are an explicitly permitted degraded success. Log known unresolved target counts rather than claiming to know every lost edge. Helper failures follow 5.2; budget exhaustion must not hide a transient or fatal Crossref failure.

Shared cross-work resolver, optional in Phase 3: collect unresolved OpenAlex IDs with their source DOIs, resolve up to 100 distinct IDs per call, and fan the DOIs back out as children. At 30 distinct targets per work with full batches, this approaches 0.3 calls per work; realized savings depend on batch fill and target overlap. Preserve the same failure and degradation rules as the per-work resolver.

Corpus handling ships in Phase 1b: request the configured `corpus=core` or `corpus=all` on singleton metadata lookups and always select `is_xpac`. In `core` mode, an `is_xpac: true` response is ineligible: record `corpus_excluded`, return `NotFound`, and continue the metadata chain. In `all` mode, accept core or expansion records after the same DOI identity check. Enforce this response check even if the upstream singleton ignores `corpus`. A singleton 404 falls through; there is no assumed expansion-only miss and no paid list retry solely to discover expansion records. Reference resolution is still paid and budget-controlled independently.

`raw_record_from_openalex_work` mirrors `raw_record_from_crossref_work` for the workflow import path, reading authorships as `RawAuthor::literal(display_name)`.

### 5.4 DataCite adapter

Base `https://api.datacite.org`, `GET /dois/{doi}?enriched=true`, `User-Agent` with `mailto:`. 404 maps to not found, 429 and 5xx to retryable, other 4xx to fatal. Mapping:

- `titles[0].title`, `descriptions` with `descriptionType` Abstract to abstract text.
- `creators` to authors, `givenName`/`familyName` when present, name otherwise, ORCID kept in raw.
- `publicationYear`, `publisher`, `types.resourceTypeGeneral`, `url`, `container`, `rightsList` and `subjects` into the record and raw.
- `relatedIdentifiers` become outbound `Reference` entries only for `References` and `Cites` where the related identifier is a DOI. `IsReferencedBy` and `IsCitedBy` go into `raw.deepref_datacite_incoming` and feed the Phase 5 inbound expansion.

`raw_record_from_datacite_work` mirrors the same pattern for the import path.

### 5.5 Worker

- `processor.rs::handle_work_fetch`: change `mark_fetching` from its boolean gate to a result that distinguishes a skipped fetch from a ready fetch carrying the run's stored chain. It already locks the run and runs once before the fetch, so read the chain under that lock. Do not change `lock_run`'s signature for its other four callers. Normalize legacy config to `["crossref"]`; for a genuinely absent stored chain or an empty allowlist intersection, use the deployment chain per 5.1.
- Build the router with the permit callback wired to `limiter::acquire(pool, provider.as_str(), rate)`, remove the outer fetch permit, and map the structured router result to the current statuses. Not found across the chain and `Rejected` become `IngestionItemStatus::NotFound`; retryable metadata or reference failures consume job attempts before terminal failure. Do not call `finalize_success` on a transient reference failure or a fatal OpenAlex reference failure.
- `workflows/nodes.rs::import_identifiers`: DOI path goes through the router using the deployment chain, because the node has no run row. Use `raw_record_from_<provider>_work`.
- `workflows.rs`: alerts stay on `search_crossref` plus PubMed.

### 5.6 Persistence touchpoints and the global cache

Keep DOI lane labels distinct from answering-provider provenance and citation-edge labels:

- `claim_doi` writes `report_fetch_state.provider='crossref'` before any provider is chosen. Keep the claim write as the lane placeholder, update the column in `finalize_success` to the metadata provider that answered, and in `finalize_terminal_failure` to the last provider attempted when structured attempt history names one. A reference helper can be that last attempted provider on failure; it must not replace metadata provenance on success.
- `is_cached` filters `f.provider='crossref'`. Drop the provider filter. The filter is vestigial today, since `claim_doi` is the only writer of that column, and it would break the cache the moment `provider` becomes the answering provider, making OpenAlex-fetched works refetch forever.
- `attach_project_work` inserts `records` with `source='crossref'` and the conflict key `(project_id, source, source_key)`. Keep `source` as the stable DOI lane label and do not repurpose it for the answering provider; `records.source` already labels lanes such as `pubmed` and import formats, and changing it would fork records per run. Provenance lives in `works.metadata_provider` and the raw payloads.
- The run source: `create_acquisition` writes `acquisition_runs.source='crossref'` and refresh copies it from the stored run. Keep it stable, for the same reason as `records.source`. `list_acquisition_items` runs the DOI query when `source='crossref'` and the PMID import query for every other value, and `refresh_acquisition` requires the same literal; relabeling an OpenAlex-first run would show an empty item list and refuse refresh. The run's chain lives in `config`, not in `source`.
- `persist_citation` explicitly writes the provider-neutral label `doi-reference` to both `fetched_citation_facts.source` and `citations.source` for new DOI-traversal edges; do not inherit the `crossref-reference` default. Those labels identify the edge's lane, not its provider. Keep truthful existing `crossref-reference` rows unchanged. On a cached attachment, read the stored fact's `source` and carry it into the project citation rather than relabelling it. Conflict handling preserves an already stored source. Actual contributing providers remain per-work in `Work.citation_provider` and raw; exact per-edge provider attribution is outside this design.

No schema migration is expected. Add disposable-Postgres coverage for each edited runtime `sqlx::query(...)`, including explicit edge-source writes and cached replay. `cargo xtask sqlx check` remains a repository gate for compile-time SQL macro metadata; it does not validate these runtime query strings. Update fixtures only for deliberate new annotations/edge labels, not the stable record/run lane values.

The cache itself is global and provider-neutral: `is_cached` and `attach_cached` reuse any fetched work by DOI across runs, projects and providers, and `attach_cached` replays its cached citations. Consequences:

- The run's chain applies to uncached DOIs only. A cached DOI keeps the provenance it was first fetched with.
- A refresh of a completed run mostly attaches cached works and fetches nothing, so decision 6's replay matters only for cold DOIs.
- Old rows keep their provider because the cache attaches them. Refresh replay only governs cold refetches.
- The Phase 1b test for run-level chain selection must use an uncached DOI.
- Transient reference failures and fatal OpenAlex reference failures never produce a fetched cache entry. Budget-degraded, `helper_fatal`, and DOI-less/missing-target successes do, and retain their degradation status on future attachments. Budget reset, a new acquisition, refresh, or rollback does not repair an already cached result. Automatic cache repair/invalidation is not introduced here.
- The union, corpus eligibility checks, paratext policy and raw fields therefore ship in Phase 1b before the OpenAlex default switch. Phase 3 consumes stored fields and may optimize resolution; it must not assume it can refetch Phase 1b works to establish correctness.

### 5.7 API

- `POST /projects/{id}/acquisitions` takes optional `metadata_providers: string[]` and no citation field in either direction. For an explicit array, trim names, deduplicate keeping first position, reject an empty array, and validate every name against the deployment's enabled set. Preserve explicit request order; do not reorder by deployment priority. A disabled or unimplemented provider is rejected with 400 naming it and the enabled set, for example `datacite is not enabled in this deployment (enabled: openalex, crossref)`. The contact e-mail check in `persist_doi_acquisition` runs first and is unchanged. An omitted field copies the deployment chain in deployment order. The singular request fields are replaced, and the generated client is regenerated in the same phase.
- New runs store that resolved chain in `config.metadata_providers`. Example: deployment `openalex,crossref` with explicit `["crossref","openalex"]` stores Crossref first; omitting the field stores OpenAlex first. `acquisition_runs.source` stays `"crossref"` for every DOI run, and the legacy `metadata_provider` and `citation_provider` run columns keep their current lane values; the run's real chain lives in `config`. `list_acquisition_items` and `refresh_acquisition` retain their existing `source` checks.
- `parse_stored_doi_config` deserializes `StoredDoiAcquisitionConfig` as an `#[serde(untagged)]` enum, both variants keeping `deny_unknown_fields`:
  - `Legacy { seed_dois, max_depth, metadata_provider, citation_provider, refresh_of? }`, where both provider strings must be `"crossref"`.
  - `Current { seed_dois, max_depth, metadata_providers, refresh_of? }`.

  `Legacy` normalizes to `Current` with `metadata_providers: ["crossref"]`. The canonical comparison rebuilds the canonical JSON in the shape that was stored, so legacy rows refresh without `ACQUISITION_REFRESH_CONFIG_INVALID`. Validate a non-empty canonical chain of known provider names without changing stored order; known-but-disabled providers remain parseable so deployment rollback can filter them at execution.
- Refresh copies the source run's stored chain into the new run's `config`, not the current default. Execution still applies the deployment allowlist: preserve the surviving stored order, or use the deployment chain if none remain. That empty-intersection fallback is an explicit exception to strict replay. The global cache caveat in 5.6 still applies; a new acquisition does not bypass cached provenance or repair cached reference degradation.

### 5.8 Web

- `ImportDoiForm.svelte` stops sending hardcoded provider fields.
- `imports.ts` labels become provider-neutral: "Not found" instead of "Not found in Crossref", including the badge variant comment.
- Fixtures and mocks that assert `metadata_provider: 'crossref'` or `source: 'crossref'` are updated in the same commit: `apps/web/tests/settings-shallow.e2e.ts`, `apps/web/tests/visual/api-fixture.ts`, `apps/web/src/routes/page.svelte.e2e.ts`, `apps/web/src/routes/settings/page.svelte.e2e.ts`, `apps/web/src/tests/mocks/handlers.ts`.
- `pnpm generate:api` runs as part of the phase that changes the contract, not only as a check.

All of 5.8 lands in Phase 1a with the contract change, before OpenAlex answers anything in Phase 1b.

### 5.9 Observability

Telemetry currently exports traces only, so this work emits tracing events and span attributes, not metrics. A metrics pipeline is a separate task.

Required events: provider attempt with provider, purpose, request count and outcome on both success and failure; fallback transition with from and to; degraded-reference success with reason/counts; budget exhaustion with reset seconds; keyless downgrade at boot or runtime. Emit core router events in Phase 1a and OpenAlex-specific events in Phase 1b, not after rollout. Separate the low-budget warning (remaining below 10 percent of the daily limit) from the exhausted-budget warning (zero remaining); deduplicate each warning per process per UTC day without suppressing per-fetch attempt records. Never log keys or credential-bearing URLs.

## 6. Phases

Each phase ends with the repository checks in section 7 for the crates it touched. Environment documentation, architecture/module descriptions and operational events land with the behavior they describe; Phase 4 is a final consistency pass, not the first time deployments learn how to configure or diagnose routing.

### Phase 0: ADR, config, enum

Deliverables: `docs/adr/0007-metadata-provider-routing.md` recording the chain, allowlist semantics, error flow, provenance and staged default; `ProviderKind` in domain; `IntegrationsConfig` fields with default `crossref`, validation tests and a boot log line. Document the new variables in `.env.example` and `docs/local-development.md` without any key value, distinguishing accepted configuration from behavior not yet available. `AcquisitionSource::DataCite` is deferred to Phase 2 as optional.

Done when: config tests cover defaults, overrides, unknown names, duplicates, empty values and a name without an adapter; `cargo nextest run -p deepref-config -p deepref-domain` passes. No behavior changes, including a missing contact e-mail: boot warns, does not refuse.

### Phase 1a: port, router, Crossref adapter, persistence, API and web

The plumbing ships with the default still `crossref`, so every change here is verifiable without a new provider answering.

Deliverables: `WorkProvider` port with the corrected `ProviderFuture` error type and structured router failure history; unused traits deleted; Crossref adapter and reference-list helper; router decision matrix, allowlist filtering, per-request permits and core tracing events; worker reads the stored chain from the revised `mark_fetching` result; import node uses the deployment chain; persistence/source-label decisions from 5.6; API ordered `metadata_providers`, enabled-set validation, legacy/current stored config and refresh replay; web changes from 5.8 plus `pnpm generate:api`. Update `docs/architecture.md` and `crates/integrations/src/lib.rs` to describe the Crossref-only router and staged defaults.

Tests: fake-provider router tests cover not found, rejected stop, exhausted retryable then next provider, fatal then next provider, fatal-only chain ending terminal, all-not-found, failure attempt history, empty filtered chain with deployment fallback and order-preserving allowlist filtering. Disposable-Postgres tests cover providerless cache reuse, finalize provider updates, explicit `doi-reference` writes, and cached replay preserving both legacy and new edge labels. HTTP tests cover ordered-array normalization, empty-array rejection, disabled-provider errors (including `openalex` in this phase), both stored config shapes, refresh replay with partial/empty allowlist intersections, and unchanged DOI item-list branching. Exercise request-order preservation with a test deployment enabling both implemented adapters once Phase 1b ships; the Crossref-only phase cannot prove a two-provider order. The imports e2e spec passes. Fixtures configure Crossref through `from_source`/`from_map`, never process environment mutation, which is unsafe under Rust 2024 and races between tests.

Done when: full workspace checks and acquisition Postgres suites pass, `pnpm generate:api:check` is clean, and router attempts/failures appear in a local Crossref-only run. Stable record/run lane values and actual Crossref metadata provenance remain unchanged; the request shape, new diagnostic annotations, per-request limiting and new edge-lane labels are intentional changes.

### Phase 1b: OpenAlex, chain openalex,crossref

Deliverables: `openalex.rs` with singleton fetching, narrowed authentication handling, boot probe, DOI/corpus checks, paratext rejection, reference resolution and unconditional Crossref union when enabled; the reference outcome/cache rules from 5.2; raw quality/enrichment fields and `raw_record_from_openalex_work`; abstract-enrichment context with shared OpenAlex key/rate state; OpenAlex-specific events and configuration documentation. Only after these ship does the default become `openalex,crossref`.

Tests: real-response fixtures cover abstract mapping, references, DOI mismatch, null DOI, empty references, `is_xpac:true` accepted in `all` and excluded in `core`, a retracted work, and `type:"paratext"` ending as not found without fall-through. Reference scenarios cover unconditional union even when all OpenAlex IDs resolve, normalized-DOI deduplication with retained Crossref bibliography, DOI-less Crossref entries, missing/DOI-less OpenAlex targets, a later-batch transient failure with no success cache, Crossref helper transient, fatal (`helper_fatal` degraded success) and 404 outcomes, and budget-exhausted/paused fetches with and without Crossref. Authentication scenarios cover keyed 401 and `403 key_not_activated` downgrade, generic 403 without downgrade, keyless authentication failure, and every boot probe outcome. Worker/Postgres scenarios prove uncached OpenAlex provenance, the stored chain winning over defaults, and persisted degraded status on later cache attachment. HTTP tests cover explicit reverse ordering, idempotent retry after a deployment reorder with the same enabled set, accepting `openalex`, and rejecting `datacite`.

Done when: full workspace checks and acquisition Postgres suites pass; a local run demonstrates reference union, paratext rejection, budget degradation and downgrade events before the default is switched. Rollback restores Crossref metadata routing with `DEEPREF_METADATA_PROVIDERS=crossref`, subject to cache and enrichment exceptions in 5.1 and 5.6.

### Phase 2: DataCite

Deliverables: `datacite.rs` plus `raw_record_from_datacite_work`; default becomes `openalex,crossref,datacite`; optional typed `AcquisitionSource::DataCite`; fixture from a real Zenodo record covering `References` and an incoming `IsReferencedBy` that must not become an edge. Update staged-default documentation and verify the existing attempt/provenance events identify DataCite.

Done when: router tests cover a DOI that only DataCite answers, the direction rule is asserted, the API accepts `datacite` in requests, and the default chain is asserted in a config test.

### Phase 3: optional shared resolver and OA consumption

Deliverables: optional shared cross-work resolver from 5.3; consume stored `deepref_oa_urls` in the open-access path in `crates/http-api/src/routes/documents.rs` ahead of Unpaywall. Reference union, corpus handling, paratext policy and raw quality/rich fields already ship in Phase 1b; do not postpone them until this phase or rely on a refetch of cached works.

OA candidates must distinguish direct PDF URLs from landing pages, use the existing external-URL validation and document-fetch path, and fall back to Unpaywall when no usable stored candidate exists. Update the route documentation with that preference order.

Tests: documents route coverage proves a usable stored OpenAlex PDF is preferred and missing/unusable candidates reach Unpaywall. If the shared resolver ships, exercise fan-out across source works and preserve the per-work resolver's retry, degradation and provenance behavior.

Done when: workspace checks pass and the documents route tests cover the OpenAlex-first OA path.

### Phase 4: final documentation and operations verification

Deliverables: reconcile the ADR, `docs/architecture.md`, integration module docs, `.env.example` and `docs/local-development.md` against the shipped chain, cache caveats and rollback semantics. Verify the events already introduced in Phases 1a/1b cover both successful and failed attempts; no operationally required documentation or event first lands here.

Done when: `pnpm check`, `pnpm test`, `pnpm lint` pass and a local traversal demonstrates the documented fallback, reference degradation, keyless downgrade and provenance outputs.

### Phase 5, optional: OpenAlex beyond metadata

Ideas to keep on the list, not scheduled: `cites:` as an inbound expansion strategy fed by the DataCite incoming relations too, OpenAlex search as an extra alert source, topics surfaced in the UI, and the paid content-download API as a PDF source. Each needs its own decision because each adds cost or new UI.

## 7. Verification commands

Standard gates from `AGENTS.md` for every phase:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo nextest run --workspace --lib --bins --locked
cargo xtask boundaries
cargo xtask sqlx check
```

Postgres suites, with both `DATABASE_URL` and `TEST_DATABASE_URL` pointing at a disposable database and `DEEPREF_REQUIRE_DATABASE=1`; retain the configured parallel PostgreSQL test group:

```bash
cargo nextest run --workspace --tests --locked
```

Web gates when the phase touches the UI or the contract:

```bash
pnpm check
pnpm test
pnpm lint
pnpm generate:api
pnpm generate:api:check
```

No schema migration is expected. `cargo xtask sqlx check` checks compile-time SQL macro metadata, not runtime `sqlx::query(...)` strings. Execute the edited claim/cache/finalization/citation queries in the disposable-Postgres suites, including legacy cached-edge replay and no success cache after transient reference failure. For actual SQL macro changes, run `cargo xtask sqlx prepare`, commit `.sqlx/`, then run `cargo xtask sqlx check` before offline builds.

Live provider smoke tests stay `#[ignore]` and run by hand: one article DOI through OpenAlex, one Zenodo DOI through DataCite, one batch reference resolution, and an expansion singleton with core/all eligibility checks. Exercise acquisition-to-persistence in a local run, including normalized reference union and cache replay. Use injectable mock endpoints for deterministic budget, authentication and later-batch failures; do not intentionally exhaust a real budget or require a real secret in fixtures.

## 8. Cost estimate and operations

Reference resolution is the only paid step in a traversal: `ceil(n / 100)` list calls per work that has resolved references, US$ 0.0001 each. A typical article with 30 references is one call, so the table below prices the common case.

| Scale | List calls | Cost | Fits |
|---|---|---|---|
| 1,000 works | 1,000 | US$ 0.10 | keyless daily budget, and 1/10 of the free key budget |
| 10,000 works | 10,000 | US$ 1.00 | exactly the free key daily budget, keyless runs out at 1,000 works |

Works with more than 100 resolved references take one extra call per 100. Singleton metadata fetches are free. The Crossref union adds a free lookup on every OpenAlex answer when Crossref is enabled; it consumes a Crossref permit even when the OpenAlex budget is healthy. Because the union runs on every OpenAlex answer, a Crossref DOI costs an OpenAlex singleton, at least one paid list call and the Crossref call, and many of the paid `ids.openalex:` lookups resolve DOIs Crossref also returns. The Crossref permit rate caps throughput either way. The optional shared resolver approaches 3,000 calls for 10,000 works at 30 distinct targets per work with full batches, US$ 0.30, raising the keyless ceiling to roughly 3,300 works per day. These are request-cost estimates, not guarantees of batch fill, unique-child counts or successful throughput.

When the budget runs out mid-traversal, keep earlier batches and perform the same Crossref union, record reference degradation, and continue if the required helper succeeds or returns 404. Without Crossref, zero/partial references are an explicitly degraded success. Such successes remain cached without automatic repair after reset; transient reference failures and fatal OpenAlex reference failures are not cached. Alerts stay on Crossref, so search cost never enters the traversal. The budget pause is per process: each replica sends paid calls until it sees its own 429 with no budget left, and keyless replicas behind one NAT share the per-IP budget.

A key is recommended but keyless deployments can fetch metadata and resolve references up to the shared daily budget. Other processes on the same IP can consume it. Pending/invalid-key boot responses or runtime keyed 401/`403 key_not_activated` downgrade to keyless until restart; generic forbidden responses do not. Never log the key or an authenticated URL.

Rollback: set `DEEPREF_METADATA_PROVIDERS=crossref` and restart API and worker with matching configuration. The deployment chain filters stored chains for jobs subsequently started; an empty intersection uses the deployment chain. This does not interrupt already executing calls, rewrite global cache entries, or prohibit OpenAlex abstract enrichment in Crossref-only mode. No migration needs sequencing.

## 9. Risks and edge cases

- OpenAlex indexes after registration. The Crossref fallback covers fresh DOIs, and alerts never move off Crossref.
- OpenAlex merges versions. The identity check in 5.3 falls through to the next provider instead of filing a sibling record, and the Zenodo case then resolves at DataCite with the exact DOI.
- `referenced_works_count` counts matched OpenAlex IDs, not the original bibliography. Phase 1b always unions Crossref references when enabled; no count comparison proves completeness.
- Budget exhaustion or missing/DOI-less targets can produce an explicitly degraded success that stays globally cached. Shared resolution reduces budget exposure; it does not repair old cached results.
- Works with no DOI on either side cannot become child jobs. They stay out of the traversal; Crossref-only entries without DOI keep landing in `project_unresolved_references`.
- Metadata outages can fall through to another provider. Exhausted transient reference failures retry the job without caching partial results; a fatal OpenAlex reference failure stops the request, while a fatal Crossref helper failure caches a degraded success. A fatal metadata failure moves to the next provider. A provider outage is not a blanket guarantee of successful degradation.
- The global cache keeps first-fetch provenance and limits run chains and newly introduced policies to uncached DOIs; see 5.6. A new acquisition, refresh or rollback does not bypass it.
- Idempotent retries compare stored config against the request's resolved config through `IdempotencyComparison::ConfigOnly`. Omitting `metadata_providers` and retrying after the deployment default changes can produce 409 `IDEMPOTENCY_KEY_REUSED`. An explicit array pins its normalized order while those providers remain enabled, even if deployment order changes. Removing a requested provider instead makes a new explicit request invalid under enabled-set validation; use the existing run for its filtered execution or submit an allowed chain with a new key.
- The Settings DTO still carries the old provider columns. Do not delete them in this work; a later migration can retire them.
- The web label change and its fixtures ship in Phase 1a, with the code, not after CI fails.

## 10. Decisions to confirm with the user

1. Chain and order `openalex,crossref,datacite` for metadata, with the staged defaults from 5.1 and the allowlist semantics. Recommended: yes.
2. Record policy: first valid metadata answer wins; OpenAlex answers always union Crossref references when deployment-enabled, regardless of counts, with OA URL collection as the other merge. Transient reference failures and fatal OpenAlex reference failures do not enter the success cache; explicit budget/target degradation may remain cached indefinitely without repair. Recommended: yes, over field-level merging.
3. Key handling: the user obtains the key and sets `DEEPREF_OPENALEX_API_KEY`; keyless operation stays valid with the budget path from 5.3; downgrade only on documented authentication failures, never generic 403. No key value in `.env.example`, fixtures or commits. Recommended: yes.
4. DOI mismatch: treat a returned sibling DOI as not found for OpenAlex and fall through. Recommended, and it turns the Zenodo case into a DataCite answer.
5. Paratext: from Phase 1b, `type == "paratext"` ends the item as not found with reason `paratext` through `Rejected`, with no fall-through. Alternative: a distinct status. Recommended: not found with reason, before the OpenAlex default switch.
6. Ordering and refresh: preserve explicit request order; refresh copies the source run's chain, then execution filters it through the deployment allowlist, falling back to the deployment chain only if none remain. Global cache hits do not refetch. Alternative: re-resolve with the current default on every refresh. Recommended: stored-order replay with the explicit allowlist fallback.
7. Rich OpenAlex fields live in raw first; UI exposure comes later. Recommended: yes.
8. Alerts stay Crossref plus PubMed. Recommended: yes.
9. Corpus default `core`, `all` only by environment; enforce core eligibility with `is_xpac` on singleton responses, not an assumed core-only endpoint or a paid list retry. Recommended: yes.
