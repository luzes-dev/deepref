# ADR 0002: Layered crate boundaries

## Status

Accepted.

## Decision

The Rust workspace follows inward dependency direction:

- `deepref-domain` owns pure identifiers, screening states, legal transition invariants, ingestion/fetch states, work/reference models, event envelopes/payloads, and in-memory graph models/algorithms. It has no HTTP, database, provider, messaging-service, or storage dependencies. Chrono, serde_json, SHA-256 and petgraph support pure data and computation, not infrastructure I/O.
- `deepref-application` owns use-case inputs such as `ScreenReportCommand`. It carries project, report, protocol, and revision context, then delegates legal transition evaluation to the domain.
- `deepref-integrations` owns Crossref, PubMed, import parsing, Unpaywall and GROBID adapters. Crossref acquisition and provider paths share one client, wire conversion and abstract enrichment implementation.
- `deepref-postgres` owns the SQLx migration set and its shared migrator.
- `deepref-http-api` owns Axum routes, request/response mapping, and SQL-backed HTTP handlers. It depends inward on application/domain code and on adapter code needed to serve the existing API.
- `apps/server` is the composition root. Its `serve`, `worker`, `all`, and `migrate` commands preserve the existing runtime wiring.

Domain and application crates must not depend on infrastructure adapters. Adapter crates may depend inward; inward crates must never depend on them. `cargo xtask boundaries` enforces the complete workspace graph through Cargo metadata, including explicit package classifications and external dependency restrictions.

## Package naming

The HTTP API package is named `deepref-http-api` and its library crate is
`deepref_http_api`. The only supported executable is the composition-root
`deepref-server` binary; API and worker deployment roles select its `serve` and
`worker` commands.

PostgreSQL migrations now live at `crates/postgres/migrations`; the HTTP API
and database-backed test fixtures use the shared `deepref-postgres` migrator.
Migration SQL remains unchanged and append-only.

The low-conflict Option C cutover replaces six packages (`core`, `events`,
`graph`, `crossref`, `providers`, `grobid`) with domain modules and one
`deepref-integrations` package, reducing 18 workspace members to 13.
Every live caller imports the owning package directly; there are no crate
aliases or migration shims. Unused catalogue models and graph-upsert code,
and the unimplemented `DocumentRepository`, `OcrEngine` and
`FullTextResolver` ports, are removed. `JobQueue`, `ReviewScheduler` and the
live document-storage port remain. Telemetry ownership is unchanged.
SQLx remains permitted in existing HTTP and worker handlers until their
use-case migration is complete.

## Consequences

Pure screening rules can be tested without infrastructure. HTTP and SQL
concerns remain available to the existing server runtime while their ownership
is explicit, and future repository or provider ports have a defined placement
without adding unused interfaces in this migration.
