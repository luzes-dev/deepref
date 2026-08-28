# Evidence identity and acquisition

## Goal

Represent scientific evidence without treating DOI as the entity identity.
Preserve every source result so PRISMA can count records before and after
resolution.

## Requirements

- A source `Record` has its own UUID and acquisition provenance.
- A deduplicated `Report` has its own UUID and may have zero or more external
  identifiers.
- A `Study` groups reports from one investigation.
- Acquisition runs record source, strategy, query or import metadata, status,
  and generated records.
- DOI, RIS, BibTeX, NBIB, CSV, and provider imports use one application-owned
  raw-record shape.
- Exact identifier matches resolve automatically when they are unambiguous.
- Fuzzy matches create reviewable proposals. They do not silently merge data.
- Resolving a record never deletes it or changes source-record counts.

## Invariants

- No public or application API uses DOI as an entity key.
- Project-scoped idempotency prevents duplicate imports.
- Identifier conflicts remain visible and cannot create a second canonical
  report through the manual path.

## Acceptance evidence

- Identity and import migrations: `crates/postgres/migrations/0006_evidence_workspace.sql`,
  `0007_evidence_identity.sql`, and `0009_acquisition_runs.sql`.
- Provider parity tests: `crates/providers/src/` and
  `crates/providers/tests/`.
- Deduplication tests: `crates/postgres/tests/deduplication.rs` and
  `crates/http-api/tests/acquisitions_postgres.rs`.
- Run:

```text
cargo test -p deepref-providers --locked
cargo test -p deepref-postgres --test deduplication --locked
cargo test -p deepref-http-api --test acquisitions_postgres --locked
```
