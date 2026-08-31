# AI foundation acceptance

This register covers the PR11 local implementation. It is not production
deployment evidence and does not claim that hosted model credentials,
PostgreSQL extensions, collectors, or model-evaluation gold sets have been
deployed.

## Accepted implementation contract

- deepref-ai exposes provider-neutral gateway, embedding, routing, task,
  grounding, persistence, prompt-registry, proposal, and policy seams.
- Structured output is parsed as external JSON, checked against the generated
  schemars schema, deserialized, and only then semantically validated.
- Every model attempt is an audit row. Failed and running attempts do not
  reserve a reuse key; completed reuse is selected deterministically by
  completion time and run id.
- Reuse hashes include task kind, resolved provider/model/version/parameters,
  rendered prompt and schema content hashes plus their versions, input,
  protocol/document hashes, and ordered evidence identity/content.
- Document blocks are active-parser-version scoped. Embeddings are retained in
  document_block_embeddings by block/model/generation/content hash with one
  deterministic current row per block. The PostgreSQL adapter never exposes
  pgvector::Vector through the AI/domain interface.
- Run evidence stores project/document scope, actual retrieval rank and score,
  and composite foreign keys reject cross-project citations.
- Tier 0 reads and tier 1 reversible metadata writes can execute; tier 2
  workflow suggestions and tier 3 scientific conclusions create proposals;
  arbitrary SQL and direct final-exclusion actions are forbidden.
- Proposals are project/entity/authority validated, unique per completed model
  run, and recovered idempotently when a completed run is reused. Resolution is
  a database compare-and-set from pending exactly once to accepted or rejected,
  with a valid actor required for resolved rows.
- Default tracing correlates opaque trace IDs only. Prompts, article text,
  PDF blocks, raw responses, and provider secrets are not emitted or persisted
  in failure metadata. When OTEL_EXPORTER_OTLP_ENDPOINT is set, the server
  installs a real OTLP batch trace exporter and flushes it at shutdown.

## Reproducible local evidence

The disposable local and integration fixture is pinned to
pgvector/pgvector:0.8.0-pg17 in infra/local/compose.yaml. The corrected
validation target used for this handoff is the explicitly named
deepref-pr11-corrected-pgvector-20260824 container at port 55435; the earlier
55434 database was not modified.

DATABASE_URL=postgres://deepref:deepref@127.0.0.1:55435/deepref cargo test -p deepref-ai

DATABASE_URL=postgres://deepref:deepref@127.0.0.1:55435/deepref cargo test -p deepref-postgres --test migration_0016 --test ai

The tests cover provider swapping without task changes, retry/reuse and
proposal recovery, content-addressed hashes, schema-before-semantic
validation, deterministic policy decisions, migration extension/index state,
legacy-row migration/backfill, versioned embedding generations,
dimension-safe hybrid retrieval, active parser and prefix filtering, scoped
evidence with rank/score round trips, and proposal CAS.

Provider credentials, model gold-set gates, and collector availability remain
deployment/evaluation concerns; exporter construction is covered locally
without requiring a collector. Runtime exporter flush and shutdown remain
deployment-lifecycle behavior.

## Architecture and evidence grounding

deepref-ai is the provider-neutral seam for structured AI work. It owns typed
task contracts, model-profile routing, immutable prompt/schema versions,
canonical input/reuse hashes, JSON Schema validation before semantic
validation, grounded evidence references, and proposal authority. The runner
hashes the rendered prompt envelope and canonical derived schema, so changing
content invalidates reuse even if a version label is accidentally unchanged.
Rig is wrapped behind a routed provider/model registry; task code never depends
on Rig, a provider SDK, SQLx, or PostgreSQL row types. AI output is never
scientific state: tier 2/3 work creates one project-scoped auditable proposal
per completed run, including recovery after a proposal write failure, and the
normal domain command remains the only state-changing authority.

PostgreSQL migration 0016_ai_foundation.sql enables vector, stores historical
document-block embedding generations with one current selection per block,
adds the active-block HNSW projection for the 1536-dimensional default route,
persists resolved model configuration and prompt/schema content hashes, and
stores project/document-scoped evidence with actual retrieval rank and score.
Failed/running attempts are not uniqueness blockers; completed reuse is
selected deterministically. Hybrid retrieval guards vector scoring by
dimension, requires the document active parser version, and treats section
filters as path prefixes. Proposal rows are project/entity/authority scoped,
unique per model run, and enforce compare-and-set resolution actors. Article
text is untrusted evidence and is JSON-encoded as data before it reaches a
model context.

Telemetry uses fmt-only tracing when no OTLP endpoint is configured and installs
a batch OTLP trace layer with flush/shutdown when one is configured.
