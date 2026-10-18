# ADR 0007: Metadata provider routing

## Status

Accepted. Implemented through Phase 4 of the [handoff](../provider-routing-handoff.md)
on this branch stack, not merged. The optional shared cross-work resolver from
section 5.3 and the typed `AcquisitionSource::DataCite` were not built. Phase 5
is not scheduled. The decisions in section 10 of the handoff were applied as
recommended, because the owner asked for the handoff to be implemented, not for
each decision to be confirmed.

## Context

DOI acquisition fetches work metadata through one hardcoded Crossref client.
The acquisition API, the workflow import node and the worker each name
`crossref` as a literal, and `IntegrationsConfig` has no provider setting.
OpenAlex answers most DOIs with more fields per call. DataCite registers the
datasets and software that neither Crossref nor OpenAlex describes fully.
Routing across them needs one decision each on order, allowlist, failure
handling and provenance.

## Decision

### Chain and vocabulary

`deepref_domain::ProviderKind` names the providers `openalex`, `crossref` and
`datacite`. Serde and `as_str` use those names. The serde rule is `lowercase`,
because `snake_case` would spell `OpenAlex` as `open_alex`.

`DEEPREF_METADATA_PROVIDERS` holds the deployment chain as a comma-separated
list. Config trims each entry and keeps the first occurrence of a duplicate. It
refuses to start on:

- an unknown name;
- an empty list, including a blank value;
- a name with no adapter in this build. The adapter set is one constant in
  `crates/config`, and it holds `openalex`, `crossref` and `datacite`.

An unset variable means the default chain, `openalex,crossref,datacite`.
`DEEPREF_OPENALEX_API_KEY` is optional. `DEEPREF_OPENALEX_CORPUS` is `core` or
`all`, and defaults to `core`. There is no switch for the reference union.

### Allowlist

The deployment chain is the allowlist. A job's effective chain is the chain
stored on its run, filtered to the enabled providers and kept in stored order.
If the filtered chain is empty, the job uses the deployment chain. A stored
chain that names a provider since removed is filtered, not refused. Changing
`DEEPREF_METADATA_PROVIDERS` therefore reaches in-flight and future jobs, so
`DEEPREF_METADATA_PROVIDERS=crossref` is the rollback.

### Error flow

One attempt is one call into an adapter. The adapter owns its retries and the
router adds none. Each outcome of an attempt routes as follows:

- `NotFound` moves to the next provider.
- `Rejected` ends the DOI as not found, with the reason in `last_error`. No
  later provider is asked. OpenAlex front matter is rejected with the reason
  `paratext`. Falling through would only change which provider records the same
  front matter, because Crossref returns it as a normal record.
- `Retryable` retries through the adapter's policy, then moves on.
- `Fatal` is a provider-level failure that retrying cannot fix. It is
  recorded and the chain moves on, so one broken provider cannot fail every
  DOI.

If every provider returned `NotFound`, the fetch is not found. If a provider
failed retryably and none answered, the fetch is retryable and the job comes
back later. A `Fatal` after an earlier retryable failure is retryable for the
same reason. If none answered and every failure was fatal, the fetch fails
terminally.

Once metadata is selected, reference failures never advance the chain. A
transient reference failure retries the job and a fatal OpenAlex reference
failure fails the request, and neither is cached. A fatal Crossref reference
helper failure is a degraded success with reason `helper_fatal`.

A DOI answered under another DOI is not found, through the identity check.

### Provenance

`Work.metadata_provider` names the provider that answered the metadata.
`Work.citation_provider` lists the reference sources in contribution order,
deduplicated, for example `openalex,crossref`. The raw payload carries
`deepref_metadata_source`, `deepref_provider_attempts`, and
`deepref_citation_sources`. An attempt names its service, its purpose
(`metadata`, `reference_resolution`, `reference_union` or `abstract_enrichment`),
the number of permits it was granted, which is its request count, and its
outcome. Failed routes keep the same history in tracing and in a bounded,
credential-free summary in `last_error`.

`citation_provider` lists only the providers whose references were kept, in
contribution order, and is empty when none were. The raw payload also carries
`deepref_reference_status`: `complete` when the reference plan finished, or
`degraded` with reasons (`budget_exhausted`, `omitted_targets`,
`doi_less_targets`, `helper_fatal`) and separate counts of retained, omitted,
DOI-less and unattempted targets. `complete` does not claim the provider captured
the whole bibliography. The record lives on the work, because the cache keeps the
answer, and a later attachment sees the same status.

OpenAlex answers also carry `deepref_oa_urls`, the open-access locations the
PDF lookup reads before Unpaywall.

The fetch cache is global and provider-neutral. A cached DOI keeps the
provenance it was first fetched with, so the run's chain applies to uncached
DOIs only.

### Port and outputs

`WorkProvider` has three methods beside the lookup. `fetch_work_after` takes the
providers that already tried the DOI in this run, so an adapter can skip work
they did. `reference_list` returns a DOI's raw reference list for the union. Its
default is a fatal error, so only providers that publish reference lists
implement it. `unions_crossref_references` says whether an accepted answer is
unioned with Crossref's list. `FetchedWork.references` is a `ReferenceReport` of
what resolution lost. An adapter takes a permit for every request it sends,
naming the service and the purpose, and the router counts the permits it was
granted per attempt. A reference failure after the metadata answered is a
distinct error, so the router never mistakes it for a metadata failure. The unused
`MetadataProvider`, `CitationProvider` and `SearchProvider` traits were deleted.

### Reference union

OpenAlex defines `referenced_works_count` as the length of `referenced_works`, so
no count shows a reference OpenAlex never matched. The router therefore merges
Crossref's reference list into every OpenAlex answer when Crossref is enabled in
the deployment, even if the run's own chain names only OpenAlex. A DOI both lists
name keeps OpenAlex's position and takes Crossref's bibliographic fields. Crossref
entries with no DOI are appended to the references, so they reach the unresolved
reference tables and replay with the cache.

The work cache is permanent, so a Crossref list that fails transiently makes the
fetch retryable instead of storing a short list. A fatal helper failure, such as
a malformed record, is a degraded success with reason `helper_fatal` that keeps
the OpenAlex references, so one bad Crossref record cannot keep a DOI out of
every project. A Crossref 404 adds nothing and keeps any existing degradation.
Budget-degraded and `helper_fatal` successes stay cached, and a reset, a refresh,
a new acquisition or a rollback does not repair them.

### Staged default

The default chain moved through these phases. Phase 2 is the default now. The work
cache never expires, so the stack ships and deploys as one unit: until the whole
corrected stack is deployed, run with `DEEPREF_METADATA_PROVIDERS=crossref`.

| Phase | Default chain |
|---|---|
| 0 | `crossref` |
| 1a | `crossref`, routed through the new port |
| 1b | `openalex,crossref` |
| 2 | `openalex,crossref,datacite` |

### Request validation

A DOI acquisition request that names a provider outside the enabled set gets 400,
and the message names the provider and the enabled set. That check runs before
the contact e-mail check in `persist_doi_acquisition`, which returns 503 as it
does today, so a request with both errors gets the 400. Names are trimmed before
they are deduplicated and checked.

### Contact e-mail

A missing `DEEPREF_CONTACT_EMAIL` is not a config error. Boot warns, and the DOI
lane is unavailable as it is today. No provider is dropped individually for a
missing address. A malformed contact e-mail still refuses to start, as before.

### Layering

`crates/config` now depends on `deepref-domain` to hold `ProviderKind`. The
boundary check forbade that edge, so `cargo xtask boundaries` now lets the
configuration layer depend on the domain layer and nothing else. The domain
owns no infrastructure, so this keeps ADR 0002's inward direction.

### Routing per fetch

The worker builds a router for each DOI fetch from the deployment chain. Every
router shares one OpenAlex client per process, so the key state and the daily
list budget belong to the process, not to the fetch.

### OpenAlex fields

Funding comes through `awards` and `funders`. The API rejects `grants` as a
select field.

### Decisions taken from the handoff

These are the section 10 recommendations, as applied:

1. The chain is `openalex,crossref,datacite`, with the staged defaults and the
   allowlist semantics above.
2. The first provider that answers wins. The reference union and the
   open-access URL collection are the only merges. There is no field-level
   merge.
3. The OpenAlex key comes from `DEEPREF_OPENALEX_API_KEY`. Keyless operation
   stays valid, with the list budget fallback. No key value appears in
   `.env.example`, fixtures or commits.
4. A DOI that OpenAlex answers under another DOI is not found, and the chain
   falls through.
5. Paratext ends the item as not found with the reason `paratext`.
6. Refresh replays the chain stored on the source run. The global cache still
   applies, so a cached DOI keeps its first provenance.
7. Rich OpenAlex fields are stored in raw. The UI does not expose them yet.
8. Alerts stay on Crossref and PubMed.
9. The corpus default is `core`, and `all` is set only through the environment.

## Consequences

- Operators pick providers with one variable and roll back with one value.
- Stored provenance and the deployment chain use the same names, so a refresh
  can replay the chain stored on its source run.
- Removing a provider from the allowlist changes which provider answers for
  runs in flight. It does not fail them.
- A blank `DEEPREF_METADATA_PROVIDERS` refuses to start. A deployment that
  interpolates an unset variable into an empty string must leave the variable
  out instead.
- Keyless OpenAlex shares its daily list budget with every process on the same
  IP. When the budget runs out, list calls stop until the reset in that process.
  Each replica sends paid calls until it sees its own 429, and the Crossref union
  covers the lost edges when Crossref is enabled.
- Every OpenAlex answer costs a Crossref lookup, and many paid `ids.openalex:`
  lookups resolve DOIs Crossref also returns. The Crossref permit rate caps
  throughput either way.
- The `crossref` literal stays as the DOI lane label on `records.source`,
  `acquisition_runs.source` and `report_fetch_state`. It names the lane, not the
  answering provider, on purpose: changing it would fork records per run, and
  relabeling an OpenAlex-first run would empty its item list and block refresh.
  Provenance lives in `works.metadata_provider` and the raw payloads.
