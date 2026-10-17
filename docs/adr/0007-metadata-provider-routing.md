# ADR 0007: Metadata provider routing

## Status

Proposed. Phase 0 is implemented: the typed chain, its config and boot log are
in place, and the default still resolves to `crossref`, so no runtime path
changes yet. The staged defaults and the decisions in section 10 of
[the handoff](../provider-routing-handoff.md) need owner confirmation before
Phase 1a.

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
  `crates/config`, and in Phase 0 only `crossref` is in it.

An unset variable means the phase's default. `DEEPREF_OPENALEX_API_KEY` is
optional. `DEEPREF_OPENALEX_CORPUS` is `core` or `all`, and defaults to `core`.

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
  later provider is asked.
- `Retryable` retries through the adapter's policy, then moves on.
- `Fatal` stops the chain for the request.

If every provider returned `NotFound`, the fetch is not found. If a provider
failed retryably and none answered, the fetch is retryable and the job comes
back later.

### Provenance

`Work.metadata_provider` names the provider that answered the metadata.
`Work.citation_provider` lists the reference sources in contribution order,
deduplicated, for example `openalex,crossref`. The raw payload carries
`deepref_metadata_source`, `deepref_provider_attempts` with one entry per
attempt and outcome, and `deepref_citation_sources`.

The fetch cache is global and provider-neutral. A cached DOI keeps the
provenance it was first fetched with, so the run's chain applies to uncached
DOIs only.

### Staged default

| Phase | Default chain |
|---|---|
| 0 | `crossref` |
| 1a | `crossref`, routed through the new port |
| 1b | `openalex,crossref` |
| 2 | `openalex,crossref,datacite` |

### Contact e-mail

A missing `DEEPREF_CONTACT_EMAIL` is not a config error. Boot warns, and the DOI
lane is refused as it is today. A malformed contact e-mail still refuses to
start, as before.

### Layering

`crates/config` now depends on `deepref-domain` to hold `ProviderKind`. The
boundary check forbade that edge, so `cargo xtask boundaries` now lets the
configuration layer depend on the domain layer and nothing else. The domain
owns no infrastructure, so this keeps ADR 0002's inward direction.

## Consequences

- Operators pick providers with one variable and roll back with one value.
- Stored provenance and the deployment chain use the same names, so a refresh
  can replay the chain stored on its source run.
- Removing a provider from the allowlist changes which provider answers for
  runs in flight. It does not fail them.
- A blank `DEEPREF_METADATA_PROVIDERS` refuses to start. A deployment that
  interpolates an unset variable into an empty string must leave the variable
  out instead.
- The `crossref` literal stays as the DOI lane label on `records.source`,
  `acquisition_runs.source` and `report_fetch_state`. It names the lane, not
  the answering provider, until the persistence phase.
