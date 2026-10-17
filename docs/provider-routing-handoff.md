# Metadata provider routing handoff

> **Status:** designed 2026-10-17. Not implemented.
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
| Persistence | `services/worker/src/store.rs` | `finalize_success` calls the `persist_work` upsert; `attach_project_work` inserts `records` rows; `claim_doi` writes `report_fetch_state.provider='crossref'` before any provider is chosen; `is_cached` filters on that same literal |
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

Verified against the provider documentation on 2026-10-17.

**OpenAlex access model**

| Operation | Cost | Limit |
|---|---|---|
| Get one work by DOI | Free, unlimited | 10 req/s without a key, 30 with a free key, 50 on annual plans |
| List plus filter, includes batch `ids.openalex:` and `cites:` filters | US$ 0.0001 per call | 100 OR values per filter, `per_page` max 100 |
| Search | US$ 0.001 per call | same per-second tiers |

Daily budget: US$ 0.10 shared per IP without a key, US$ 1 with a free key. A 429 covers both per-second limiting and budget exhaustion. Read `X-RateLimit-Remaining` to tell them apart: zero means the daily budget is gone until the reset reported in `X-RateLimit-Reset`, seconds until midnight UTC; above zero means slow down. Error responses are free. Cursor paging needs a key.

Keys have three states, confirmed against the live API on 2026-10-17: a valid key, a pending key that returns 403 `key_not_activated` until its owner clicks the activation link, and an invalid or revoked key that returns 401 `Invalid or missing API key` on every endpoint, including the usage one.

**OpenAlex corpus**

Core is the default and already includes Crossref, PubMed, DataCite and repository records: 320M+ works. The expansion adds about 190M dataset and repository records without a core match, reachable with `corpus=all`. New works always land in core.

**OpenAlex fields worth using**

`referenced_works` holds references resolved to OpenAlex IDs only, so it can be shorter than `referenced_works_count`. `ids` carries PMID, PMCID and MAG IDs. `best_oa_location` and `locations` carry PDF URLs, licenses and versions from the Unpaywall data. `is_retracted` and `is_paratext` flag quality. `authorships` carries institutions, ORCID and corresponding flags. Topics, keywords, grants, `counts_by_year` and `related_works` are extras for the raw payload.

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
| Enrichment fields | OpenAlex | Topics, institutions, grants, retraction and citation counts are first-class |

Priority by operation:

- Metadata by DOI: `openalex` then `crossref` then `datacite`.
- References: OpenAlex `referenced_works` resolved in batches, unioned with Crossref reference lists when OpenAlex lost entries or ran out of budget, and with DataCite `References`/`Cites` relations when DataCite answered.
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

The configured chain is also the deployment allowlist:

- A job's effective chain is the chain stored on its run, filtered to enabled providers, preserving the stored order.
- Enabled means named in the deployment chain and implemented by an adapter.
- If the filtered chain is empty, the job uses the deployment chain. Config validation keeps that chain non-empty.
- A stored chain that names a provider later removed from the allowlist is filtered out, not an error. This is what makes `DEEPREF_METADATA_PROVIDERS=crossref` a real rollback for in-flight and future jobs, not only for new runs.

Config errors still refuse to start: unknown name, a name with no adapter yet, or an empty configured list. A missing contact e-mail is not a config error; see 5.2.

Rules: trim entries, drop duplicates keeping first position, reject an empty or unknown list with `ConfigError::Invalid`, and reject a name that has no adapter yet with a clear message. A missing key is valid. Log the effective chain once at API and worker boot.

Rate values: OpenAlex 25 per second with a key and 8 per second after a keyless downgrade, DataCite 3 per second, constants in `crates/integrations`. Crossref keeps `settings.rate_limit_per_second`.

`ProviderKind` lives in `crates/domain/src/provider.rs` with `FromStr`, `as_str` and snake_case serde.

The user obtains the OpenAlex key from the OpenAlex settings page. The key never lands in `.env.example`, fixtures, or commits.

### 5.2 Provider layer

The port lives in `crates/application`, per ADR 0002, and `crates/integrations` implements it:

```rust
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

Delete `MetadataProvider`, `CitationProvider` and `SearchProvider` and their Crossref impls in the same phase. `RawRecord` and `ProviderFuture` stay; the import parsers use them.

Attempt semantics: one attempt is one call into an adapter, which may make one HTTP request plus its own internal retries. The adapter keeps the current policy fed by `settings.retry_attempts`. The router adds no retry loop of its own: it moves to the next provider on `NotFound`, or after the adapter exhausts its retries on `Retryable`. Job-level retries stay as they are. No third layer.

Permits: every outbound request takes a permit, including the paid OpenAlex reference resolution and the Crossref reference helper from 5.3. The permit callback is threaded from the worker down to the adapters, so paid calls inside an adapter ask for the same provider's permit.

Flow rules: `NotFound` moves to the next provider. `Rejected` ends the chain for that DOI on the spot and maps to `IngestionItemStatus::NotFound` with the reason in `last_error`; it is not a job failure and no later provider is asked. `Retryable` retries the same provider through its own policy, then moves on. `Fatal` stops the chain for that request. If every provider returned `NotFound`, the fetch is not found. If any provider failed retryably and none succeeded, the fetch is retryable so the job comes back later.

The contact e-mail is required for the whole DOI lane, not per provider. Every adapter sends it as `mailto`, and `persist_doi_acquisition` already refuses DOI acquisitions, create and refresh, when it is empty. Without it, the API and worker boot, warn once, and leave the DOI lane unavailable: the API keeps its current `DEEPREF_CONTACT_EMAIL` error, and worker jobs and the import node fail with the current message, exactly as they do today. No provider is dropped individually for a missing e-mail. Phase 0 stays behavior-neutral this way.

An OpenAlex key that fails is not a reason to disable OpenAlex: the adapter downgrades to keyless per 5.3.

Provenance: `Work.metadata_provider` names the provider that answered the metadata fetch. `Work.citation_provider` records the reference sources in contribution order, deduplicated, for example `openalex` or `openalex,crossref`. Raw carries `deepref_metadata_source`, `deepref_provider_attempts` with one entry per attempt and outcome, and `deepref_citation_sources`.

Abstract enrichment: when the chain already tried OpenAlex for this DOI, the Crossref fallback disables OpenAlex inside the enricher and still tries Europe PMC.

### 5.3 OpenAlex adapter

Base `https://api.openalex.org`, identification via `User-Agent` and `mailto` from `contact_email`, `select=` per fetch shape, base URL injectable for tests.

Mapping to `WorkWithReferences`:

- `display_name` to title, `abstract_inverted_index` reconstructed to plain text through the existing `clean_abstract`.
- `publication_year` and `publication_date` to year fields, `type` to work type.
- `primary_location.source.display_name` to container title, `host_organization_name` to publisher.
- `cited_by_count` to total citations, `referenced_works_count` to references count.
- `best_oa_location` and `locations` into `raw.deepref_oa_urls`.
- The whole OpenAlex body into `raw`, plus `is_retracted`, `is_paratext`, `ids`, `topics` and `authorships` as they come.

Status table:

| Response | Meaning | Handling |
|---|---|---|
| 200 | Work found | Run the identity check below |
| 301 | Entity merged, followed by the HTTP client | The identity check decides |
| 404 | DOI unknown | `NotFound`, next provider |
| 429 on a singleton | Per-second limit; singleton calls are free | `Retryable` with backoff |
| 429 on a list or search, `X-RateLimit-Remaining` above zero | Per-second limit | `Retryable` with backoff |
| 429 with `X-RateLimit-Remaining` at zero | Daily budget exhausted | Budget path below, not a retry |
| 401 or 403 at runtime | Key invalid, revoked, or pending | Warn once, mark the key unusable in this process, retry the request keyless, continue keyless until restart |
| 5xx or network failure | Temporary | `Retryable` |

Boot probe: one call to `GET /rate-limit` with the key, expected free. 200 means keyed mode. 401, or 403 `key_not_activated`, means warn once and continue keyless, with the lower rate. A network failure at boot keeps the key configured and logs a warning; the runtime downgrade covers it when the API becomes reachable. A key activated after boot takes effect on the next restart.

Identity check: compare the DOI OpenAlex returns with the requested one through `normalize_doi`. A mismatch, including a null DOI, makes OpenAlex answer not found for that request, logged with both DOIs, and the chain falls through. A sibling version must never be filed under the requested DOI.

References: resolve `referenced_works` with `ceil(n / 100)` list calls per work, `filter=ids.openalex:W1|...|W100&select=id,doi&per_page=100`. A typical article with 30 resolved references is one call; a review with 250 is three. Only targets with a DOI become `Reference { doi: Some(..) }`.

Budget exhaustion: when a list call returns 429 with `X-RateLimit-Remaining` at zero, stop issuing list calls in this process until `X-RateLimit-Reset`, and record the outcome in `deepref_provider_attempts` as `budget_exhausted` with a warning. The metadata still comes from OpenAlex, since singleton fetches are free. For references, the router asks the Crossref adapter for the DOI's reference list when Crossref is enabled, which is a free Crossref lookup, and merges the result into `references`. When Crossref is not enabled, the work keeps zero or partial references, the attempt record stays, and the warning names the lost edges. The helper ships with the Crossref adapter in Phase 1a, OpenAlex uses it for budget exhaustion from Phase 1b, and Phase 3 extends it to the lost-edge union.

Shared cross-work resolver, optional in Phase 3: collect unresolved OpenAlex IDs with their source DOIs, resolve up to 100 distinct IDs per call, and fan the DOIs back out as children. This drops the per-work cost to roughly a third of a call and raises the keyless ceiling.

`corpus=all` handling: singleton lookups cover core. When a singleton misses and the corpus is `all`, retry once through a list call `works?filter=doi:{doi}&corpus=all&select=...`. When list calls are stopped for budget exhaustion, the retry does not run: OpenAlex answers not found for that DOI, the chain falls through, and the `budget_exhausted` attempt record from the earlier call remains the explanation.

`raw_record_from_openalex_work` mirrors `raw_record_from_crossref_work` for the workflow import path, reading authorships as `RawAuthor::literal(display_name)`.

### 5.4 DataCite adapter

Base `https://api.datacite.org`, `GET /dois/{doi}?enriched=true`, `User-Agent` with `mailto:`. 404 maps to not found, 429 and 5xx to retryable, other 4xx to fatal. Mapping:

- `titles[0].title`, `descriptions` with `descriptionType` Abstract to abstract text.
- `creators` to authors, `givenName`/`familyName` when present, name otherwise, ORCID kept in raw.
- `publicationYear`, `publisher`, `types.resourceTypeGeneral`, `url`, `container`, `rightsList` and `subjects` into the record and raw.
- `relatedIdentifiers` become outbound `Reference` entries only for `References` and `Cites` where the related identifier is a DOI. `IsReferencedBy` and `IsCitedBy` go into `raw.deepref_datacite_incoming` and feed the Phase 5 inbound expansion.

`raw_record_from_datacite_work` mirrors the same pattern for the import path.

### 5.5 Worker

- `processor.rs::handle_work_fetch`: `mark_fetching` already locks the run and runs once before the fetch, so it returns the run's stored chain. Do not change `lock_run`'s signature for its other four callers. The effective chain is that stored chain filtered per 5.1, falling back to the deployment chain when the run has none, which covers legacy rows and in-flight events.
- Build the router with the permit callback wired to `limiter::acquire(pool, provider.as_str(), rate)`, and map router outcomes to the current terminal statuses. Not found across the chain becomes `IngestionItemStatus::NotFound`; retryable drains attempts before the terminal failure, as today.
- `workflows/nodes.rs::import_identifiers`: DOI path goes through the router using the deployment chain, because the node has no run row. Use `raw_record_from_<provider>_work`.
- `workflows.rs`: alerts stay on `search_crossref` plus PubMed.

### 5.6 Persistence touchpoints and the global cache

Four writes name `'crossref'` as a lane label:

- `claim_doi` writes `report_fetch_state.provider='crossref'` before any provider is chosen. Keep the claim write as the lane placeholder, and update the column in `finalize_success` to the provider that answered, and in `finalize_terminal_failure` to the last provider attempted when the error names one. This keeps the column honest without a migration.
- `is_cached` filters `f.provider='crossref'`. Drop the provider filter. The filter is vestigial today, since `claim_doi` is the only writer of that column, and it would break the cache the moment `provider` becomes the answering provider, making OpenAlex-fetched works refetch forever.
- `attach_project_work` inserts `records` with `source='crossref'` and the conflict key `(project_id, source, source_key)`. Keep `source` as the stable DOI lane label and do not repurpose it for the answering provider; `records.source` already labels lanes such as `pubmed` and import formats, and changing it would fork records per run. Provenance lives in `works.metadata_provider` and the raw payloads.
- The run source: `create_acquisition` writes `acquisition_runs.source='crossref'` and refresh copies it from the stored run. Keep it stable, for the same reason as `records.source`. `list_acquisition_items` runs the DOI query when `source='crossref'` and the PMID import query for every other value, and `refresh_acquisition` requires the same literal; relabeling an OpenAlex-first run would show an empty item list and refuse refresh. The run's chain lives in `config`, not in `source`.

No schema migration is expected for these changes, but each query edit needs `cargo xtask sqlx check` and the existing acquisition fixtures updated where they assert the old literal.

The cache itself is global and provider-neutral: `is_cached` and `attach_cached` reuse any fetched work by DOI across runs, projects and providers, and `attach_cached` replays its cached citations. Consequences:

- The run's chain applies to uncached DOIs only. A cached DOI keeps the provenance it was first fetched with.
- A refresh of a completed run mostly attaches cached works and fetches nothing, so decision 6's replay matters only for cold DOIs.
- Old rows keep their provider because the cache attaches them. Refresh replay only governs cold refetches.
- The Phase 1b test for run-level chain selection must use an uncached DOI.

### 5.7 API

- `POST /projects/{id}/acquisitions` takes `metadata_providers: string[]` and no citation field in either direction. Every explicit name must be in the deployment's enabled set from 5.1; anything else, including a provider whose adapter ships in a later phase such as `openalex` during Phase 1a or `datacite` during Phase 1b, is rejected with 400 and a message naming the provider and the enabled set, for example `datacite is not enabled in this deployment (enabled: openalex, crossref)`. The contact e-mail check in `persist_doi_acquisition` runs first and is unchanged. Requests that omit the field use the effective chain, so the worker's stored-chain fallback only appears when a later deployment change removes a provider from a run created earlier. The singular string fields are replaced, so the generated client is regenerated in the same phase.
- New runs store the effective chain in `config.metadata_providers`, preserving deployment order. `acquisition_runs.source` stays the literal `"crossref"` for every DOI run, and the legacy `metadata_provider` and `citation_provider` run columns keep their current lane values, because nothing reads them and the run's real chain is in `config`. `list_acquisition_items` and `refresh_acquisition` keep their existing `source` checks.
- `parse_stored_doi_config` deserializes `StoredDoiAcquisitionConfig` as an `#[serde(untagged)]` enum, both variants keeping `deny_unknown_fields`:
  - `Legacy { seed_dois, max_depth, metadata_provider, citation_provider, refresh_of? }`, where both provider strings must be `"crossref"`.
  - `Current { seed_dois, max_depth, metadata_providers, refresh_of? }`.

  `Legacy` normalizes to `Current` with `metadata_providers: ["crossref"]`. The canonical comparison rebuilds the canonical JSON in the shape that was stored, so legacy rows refresh without `ACQUISITION_REFRESH_CONFIG_INVALID`. Every name in the chain is validated against the known providers.
- Refresh replays the chain stored on the source run, never the current default, subject to the global cache caveat in 5.6. The refreshed run stores that chain in its own `config`. Moving existing rows to OpenAlex is a new acquisition or a later explicit feature, not a side effect of refresh.

### 5.8 Web

- `ImportDoiForm.svelte` stops sending hardcoded provider fields.
- `imports.ts` labels become provider-neutral: "Not found" instead of "Not found in Crossref", including the badge variant comment.
- Fixtures and mocks that assert `metadata_provider: 'crossref'` or `source: 'crossref'` are updated in the same commit: `apps/web/tests/settings-shallow.e2e.ts`, `apps/web/tests/visual/api-fixture.ts`, `apps/web/src/routes/page.svelte.e2e.ts`, `apps/web/src/routes/settings/page.svelte.e2e.ts`, `apps/web/src/tests/mocks/handlers.ts`.
- `pnpm generate:api` runs as part of the phase that changes the contract, not only as a check.

All of 5.8 lands in Phase 1a with the contract change, before OpenAlex answers anything in Phase 1b.

### 5.9 Observability

Telemetry currently exports traces only, so this work emits tracing events and span attributes, not metrics. A metrics pipeline is a separate task.

Events worth emitting: provider attempt with provider, purpose and outcome; fallback transition with from and to; budget exhaustion with reset seconds; keyless downgrade at boot or at runtime. The budget warning fires at most once per process per UTC day, guarded by an in-memory marker, when `X-RateLimit-Remaining` falls below 10 percent.

## 6. Phases

Each phase ends with the repository checks in section 7 for the crates it touched.

### Phase 0: ADR, config, enum

Deliverables: `docs/adr/0007-metadata-provider-routing.md` recording the chain, allowlist semantics, error flow, provenance and the staged default; `ProviderKind` in domain; `IntegrationsConfig` fields with default `crossref`, validation tests, and a boot log line for the effective chain. `AcquisitionSource::DataCite` is deferred to Phase 2 as optional.

Done when: config tests cover defaults, overrides, unknown names, duplicates, empty values and a name without an adapter; `cargo nextest run -p deepref-config -p deepref-domain` passes. No behavior changes, including a missing contact e-mail: boot warns, does not refuse.

### Phase 1a: port, router, Crossref adapter, persistence, API and web

The plumbing ships with the default still `crossref`, so every change here is verifiable without a new provider answering.

Deliverables: `WorkProvider` port in application with `ProviderFetchError` including `Rejected`, unused traits deleted; Crossref adapter behind the port, including the reference-list helper; `router.rs` with the decision matrix, allowlist filtering, attempt semantics and permit threading; worker reads the chain from `mark_fetching` and fetches through the router; import node through the router with the deployment chain; persistence decisions from 5.6; API `metadata_providers` array, enabled-set validation, untagged stored config, refresh replay; web changes from 5.8 plus `pnpm generate:api`.

Tests: router unit tests with fake providers for not found, a rejected answer ending the chain as not found, retryable then next provider, fatal stop, all-not-found, empty filtered chain with deployment fallback, and a stored chain filtered by the allowlist; worker test proving the cache still attaches a Crossref-era row; persistence tests for the finalize provider update and the providerless `is_cached`; http-api tests for array validation, 400 for a provider outside the enabled set and for `openalex` in this phase, both stored config shapes, refresh replay, and the item list still branching on `source='crossref'`; e2e imports spec passes. Fixture tests that assert Crossref provenance build the config through the `from_source` or `from_map` closure with `metadata_providers = ["crossref"]`, the way the config tests already do, never through process environment mutation, which is unsafe under Rust 2024 and races between tests.

Done when: full workspace checks pass, the acquisition postgres suite passes with existing provenance assertions unchanged, and `pnpm generate:api:check` is clean. Crossref-only acquisitions behave as before apart from the request shape.

### Phase 1b: OpenAlex, chain openalex,crossref

Deliverables: `openalex.rs` with singleton fetch, status table handling, boot probe, identity check, reference resolution with `ceil(n/100)` calls, budget fallback through the Crossref reference helper, and `raw_record_from_openalex_work`; the abstract enricher skips OpenAlex on a Crossref fallback; default becomes `openalex,crossref`.

Tests: fixtures from real OpenAlex responses, including abstract, referenced works, a DOI mismatch, a work without references, a budget-exhausted 429, a runtime 401 downgrade and each boot probe outcome; worker test proving OpenAlex provenance on an uncached DOI; worker test proving the run-level chain wins over the default for an uncached DOI; http-api test accepting `openalex` and rejecting `datacite`.

Done when: full workspace checks pass and the acquisition postgres suite passes with the chain configured. Rollback to Phase 1a behavior is `DEEPREF_METADATA_PROVIDERS=crossref`.

### Phase 2: DataCite

Deliverables: `datacite.rs` plus `raw_record_from_datacite_work`; default becomes `openalex,crossref,datacite`; optional typed `AcquisitionSource::DataCite`; fixture from a real Zenodo record covering `References` and an incoming `IsReferencedBy` that must not become an edge.

Done when: router tests cover a DOI that only DataCite answers, the direction rule is asserted, the API accepts `datacite` in requests, and the default chain is asserted in a config test.

### Phase 3: reference union, shared resolver, OA, rich fields, corpus

Deliverables: the Phase 1a Crossref reference helper extended to the lost-edge union on OpenAlex answers when the resolved count is below `referenced_works_count`, config-gated and on by default; optional shared cross-work resolver from 5.3; `deepref_oa_urls` consumed by the open-access path in `crates/http-api/src/routes/documents.rs` ahead of Unpaywall; retraction and paratext flags stored; `deepref_oa_urls`, topics, institutions, grants and citation counts stored in raw; `corpus=all` path through a list call.

Paratext ends the item at OpenAlex through the `Rejected` outcome from 5.2 with reason `paratext`, which maps to not found and never reaches another provider. Falling through would not filter anything, because Crossref returns the same front matter as a normal record, so it would only change which provider gets recorded.

Tests: union dedupes DOIs; a retracted work carries the flag; a paratext work ends as not found with the reason; OA lookup prefers the OpenAlex URL when present; budget exhaustion produces the Crossref fallback and the attempt record.

Done when: workspace checks pass and the documents route tests cover the OpenAlex-first OA path.

### Phase 4: docs, observability, cleanup

Deliverables: tracing events from 5.9; `docs/architecture.md` paragraph that says "Crossref DOI acquisition, metadata and citation adapters use one client" is rewritten for the chain; `crates/integrations/src/lib.rs` module docs; `.env.example` and `docs/local-development.md` list the new variables without the key value.

Done when: `pnpm check`, `pnpm test`, `pnpm lint` pass and the tracing events appear in a local run.

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

Postgres suites, against a disposable database with `DEEPREF_REQUIRE_DATABASE=1`:

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

No schema migration is expected. The three `store.rs` query edits still run through `cargo xtask sqlx check`, which is the gate that catches drift.

Live smoke tests against the real providers stay `#[ignore]` and are run by hand: one article DOI through OpenAlex, one Zenodo DOI through DataCite, one batch reference resolution.

## 8. Cost estimate and operations

Reference resolution is the only paid step in a traversal: `ceil(n / 100)` list calls per work that has resolved references, US$ 0.0001 each. A typical article with 30 references is one call, so the table below prices the common case.

| Scale | List calls | Cost | Fits |
|---|---|---|---|
| 1,000 works | 1,000 | US$ 0.10 | keyless daily budget, and 1/10 of the free key budget |
| 10,000 works | 10,000 | US$ 1.00 | exactly the free key daily budget, keyless runs out at 1,000 works |

Works with more than 100 resolved references take one extra call per 100. Singleton fetches are free, so metadata never consumes budget. The shared cross-work resolver in 5.3 cuts the common case to about 3,000 calls for 10,000 works, US$ 0.30, and raises the keyless ceiling to roughly 3,300 works per day. It is optional in Phase 3.

When the budget runs out mid-traversal, 5.3 takes references from the Crossref reference helper, records the outcome, and keeps the run moving; child discovery for those works is reduced but not silently lost. Alerts stay on Crossref, so search cost never enters the traversal.

A key is recommended but a keyless deployment can fetch metadata and resolve references up to the daily budget. Without a key the budget is shared by everything on the machine's IP, so other processes can consume it. If the key is pending or invalid, the boot probe downgrades to keyless with a warning, and a runtime 401 or 403 does the same until restart.

Rollback is one environment change: `DEEPREF_METADATA_PROVIDERS=crossref`. The deployment chain is the allowlist, so the change filters stored chains too and covers in-flight and future jobs, not only new runs. Deploy API and worker together; no migration to sequence.

## 9. Risks and edge cases

- OpenAlex indexes after registration. The Crossref fallback covers fresh DOIs, and alerts never move off Crossref.
- OpenAlex merges versions. The identity check in 5.3 falls through to the next provider instead of filing a sibling record, and the Zenodo case then resolves at DataCite with the exact DOI.
- `referenced_works` is incomplete by definition. Phase 3 bounds the union request to works where edges were lost.
- Budget exhaustion mid-traversal reduces child discovery; the attempt record makes it visible. The shared resolver lowers the exposure.
- Works with no DOI on either side cannot become child jobs. They stay out of the traversal; Crossref-only entries without DOI keep landing in `project_unresolved_references`.
- A provider outage during a traversal degrades, not fails: retryable drains into a later job attempt, and the next provider answers when it can.
- The global cache keeps first-fetch provenance and limits the run chain to uncached DOIs; see 5.6.
- Idempotent retries compare the stored config against the request's resolved config through `IdempotencyComparison::ConfigOnly`. A client that omits `metadata_providers` and retries the same key after a deploy changes the default chain gets 409 `IDEMPOTENCY_KEY_REUSED`. Sending the chain explicitly pins it; a new key is the other way out.
- The Settings DTO still carries the old provider columns. Do not delete them in this work; a later migration can retire them.
- The web label change and its fixtures ship in Phase 1a, with the code, not after CI fails.

## 10. Decisions to confirm with the user

1. Chain and order `openalex,crossref,datacite` for metadata, with the staged defaults from 5.1 and the allowlist semantics. Recommended: yes.
2. Record policy: first provider that answers wins, with reference union and OA URL collection as the only merges. Recommended: yes, over field-level merging.
3. Key handling: the user obtains the key and sets `DEEPREF_OPENALEX_API_KEY`; keyless operation stays valid with the budget fallback from 5.3; no key value in `.env.example`, fixtures or commits. Recommended: yes.
4. DOI mismatch: treat a returned sibling DOI as not found for OpenAlex and fall through. Recommended, and it turns the Zenodo case into a DataCite answer.
5. Paratext: end the item as not found with reason `paratext` through the `Rejected` outcome, no fall-through, because fall-through only changes which provider records the same front matter. Alternative: a distinct status. Recommended: not found with reason.
6. Refresh: replay the chain stored on the source run, subject to the global cache caveat. Alternative: re-resolve with the current default, which would silently switch old rows. Recommended: replay.
7. Rich OpenAlex fields live in raw first; UI exposure comes later. Recommended: yes.
8. Alerts stay Crossref plus PubMed. Recommended: yes.
9. Corpus default `core`, `all` only by environment. Recommended: yes.
