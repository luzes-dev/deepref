# Documents, studies, and appraisal

## Goal

Connect metadata to full text, group reports into investigations, and assess
trustworthiness without changing eligibility.

## Requirements

- Documents have explicit missing, external, uploaded, retrieving, available,
  and failed states.
- Storage works through a document-store boundary with local filesystem and
  S3-compatible implementations.
- Uploads are streamed and content-addressed.
- PDF parsing produces versioned pages and evidence blocks with coordinates.
- Full-text queues expose missing documents and failed retrievals.
- Reports can be assigned to, renamed within, and removed from studies.
- Study membership changes are reversible and audited.
- Study design classification suggests appraisal tools.
- Appraisal definitions are schema-driven and versioned.
- Appraisal responses and evidence references are validated before completion.

## Invariants

- Full-text exclusion has exactly one primary reason.
- Appraisal never changes screening eligibility.
- Approved extraction and appraisal evidence keeps its document, page, block,
  parser-version, and content-hash provenance.
- A study may contain multiple reports from one investigation.

## Acceptance evidence

- Document code and migrations:
  `crates/documents/`, `crates/postgres/migrations/0013_documents_full_text.sql`,
  and `services/worker/tests/document_parse.rs`.
- Study and appraisal tests:
  `crates/postgres/tests/study_appraisal.rs`,
  `crates/http-api/tests/study_postgres.rs`, and the appraisal and extraction
  Playwright suites.
- Run:

```text
cargo test -p deepref-postgres --test study_appraisal --locked
cargo test -p deepref-worker --test document_parse --locked
pnpm --filter @deepref/web test:e2e -- --grep 'appraisal|extraction|study'
```
