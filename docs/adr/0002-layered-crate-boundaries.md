# ADR 0002: Layered crate boundaries

## Status

Accepted.

## Decision

The Rust workspace follows inward dependency direction:

- `deepref-domain` owns pure identifiers, screening states, and legal transition invariants. It has no HTTP, database, provider, messaging, graph, or storage dependencies.
- `deepref-application` owns use-case inputs such as `ScreenReportCommand`. It carries project, report, protocol, and revision context, then delegates legal transition evaluation to the domain.
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

## Consequences

Pure screening rules can be tested without infrastructure. HTTP and SQL
concerns remain available to the existing server runtime while their ownership
is explicit, and future repository or provider ports have a defined placement
without adding unused interfaces in this migration.
