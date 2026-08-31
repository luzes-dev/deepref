# PRISMA and graph projections

## Goal

Derive reporting and graph views from persisted evidence decisions and
provenance.

## Requirements

- PRISMA counts are calculated from records, resolutions, retrieval states,
  screening events, exclusion reasons, and study membership.
- Exports include CSV, JSON, RIS, BibTeX, SVG, PNG, audit CSV, and protocol
  snapshots.
- Export output is deterministic for the same database state.
- PostgreSQL remains the graph source of truth.
- Graph projections use stable UUID nodes and typed edges.
- Screening, study, appraisal, provenance, and metrics overlays affect
  visualization only.
- Citation rank can prioritize inspection but cannot decide eligibility.

## Invariants

- No manual count or stale snapshot is authoritative.
- Graph failure cannot invalidate core review state.
- Exported audit rows retain actor, event, protocol, and provenance identity.

## Acceptance evidence

- Projection and PRISMA tests:
  `crates/postgres/tests/prisma.rs`, `crates/postgres/tests/graph.rs`, and
  `crates/http-api/tests/exports_postgres.rs`.
- UI projection code:
  `apps/web/src/lib/components/project/graph-overlays.ts` and
  `apps/web/src/lib/features/prisma/`.
- Run:

```text
cargo test -p deepref-postgres --test prisma --locked
cargo test -p deepref-postgres --test graph --locked
cargo test -p deepref-http-api --test exports_postgres --locked
```
