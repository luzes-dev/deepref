# DeepRef architecture

DeepRef maps article citation networks from seed DOIs. The SvelteKit SPA uses a same-origin `/api` contract through the web gateway. A Rust Axum API owns HTTP behavior; the Rust worker performs durable recursive ingestion and reconciliation; PostgreSQL is authoritative for state, jobs, graph facts, and metrics.

## Data and event planes

PostgreSQL owns projects, ingestions/items, works, project membership, canonical fetched-reference facts, citations, unresolved references, claims/leases, domain events, durable jobs, graph status, and metric snapshots. The v2 evidence workspace is layered alongside the citation-ingestion model: records, reports, studies, screening events/state, review events, documents, audit rows, and leased jobs are all PostgreSQL tables.

Authoritative state changes and their follow-up jobs are committed in one transaction. The worker claims jobs with `FOR UPDATE SKIP LOCKED`, owns a lease with expiry renewal, retries failures with persisted attempts, marks terminal failures dead, and reclaims expired work. Stable dedupe keys prevent duplicate logical jobs. Ingestion preserves cached/fetched facts, discovered children, citations, events, and completion in the same transaction boundary; reconciliation repairs already-committed rows without silently dropping work.

Graph reads use canonical UUIDs from `reports`, `project_reports`, and `citations`. Reports without DOI identifiers remain graph nodes. Responses are deterministic and bounded; edges are UUID-to-UUID and never depend on DOI mapping. Project metrics use the same degree/rank semantics as the legacy fixture and are recomputed after imports and later ingestions. The PostgreSQL graph status endpoint reports metric/projection freshness for compatibility with existing clients; it is not an external graph service.

The v2 screening command writes its event, state projection, and project lifecycle status in one transaction. PRISMA is a live deterministic projection of canonical relational state; no recomputation job or snapshot is authoritative. The command uses optimistic revisions and returns `409 screening_revision_conflict` when another browser, worker, or automation changes the same report.

## AI foundation and evidence grounding

`deepref-ai` is the provider-neutral seam for structured AI work. It owns typed
task contracts, model-profile routing, immutable prompt/schema versions,
canonical input/reuse hashes, JSON Schema validation before semantic
validation, grounded evidence references, and proposal authority. Rig is
wrapped at the gateway seam; task code never depends on Rig, a provider SDK,
SQLx, or PostgreSQL row types. AI output is never scientific state: tier 2/3
work creates an auditable proposal and the normal domain command remains the
only state-changing authority.

PostgreSQL migration `0016_ai_foundation.sql` enables `vector`, stores
versioned document-block embeddings, adds the active-block HNSW projection for
the 1536-dimensional default route, persists resolved model configuration and
evidence references for every run, and enforces compare-and-set proposal
resolution. Hybrid retrieval combines PostgreSQL FTS and cosine similarity with
deterministic section/kind filters and tie-breaking. Article text is untrusted
evidence and is fenced as data before it reaches a model context.

The default local fixture is the pinned `pgvector/pgvector:0.8.0-pg17` image.
This is disposable migration/integration tooling; production extension and
backup choices remain deployment-owned. The local acceptance evidence is
tracked in [AI foundation acceptance](acceptance/ai-foundation.md).

## Corrected AI foundation details

Every AI attempt is an audit row. Failed and running rows do not reserve a
reuse key; completed reuse is selected deterministically. The runner hashes the
rendered prompt envelope and canonical derived schema in addition to their
immutable version labels, and proposal creation is idempotent and recovered
when a completed run is reused.

Embeddings are historical rows keyed by block, model, generation, and content
hash, with one current selection per block. Retrieval guards vector scoring by
dimension, requires the document active parser version, and treats section
filters as path prefixes. Run evidence carries project/document scope and the
actual rank and score, with composite database constraints preventing
cross-project citations. Policy input is typed and includes actor, project,
tool, authority, action, arguments, and project capabilities; scientific work
always becomes a proposal. Telemetry is fmt-only without an endpoint and uses
an OTLP batch layer with flush/shutdown when configured.

`deepref-review` compiles consequential review semantics before execution. Its
closed catalog binds checked-in workflow, prompt, schema, policy, and parser
assets to typed review subjects, then derives the run manifest and node input
fingerprints from actual content hashes. HTTP, assistant, and automation callers
therefore share one interface while `deepref-ai` remains responsible for a
single provider-neutral structured model call. See
[ADR 0004](adr/0004-compiled-review-definition-seam.md).

PostgreSQL stores one immutable compiled manifest per review run, immutable
step attempts, content-addressed artifacts and predecessor lineage, accepted
attempt pointers, and expert calibration bundles. Automation-triggered runs
are admitted only when the selected bundle is passing and exactly matches the
manifest semantic hash. HTTP and assistant callers receive an asynchronous
review-run resource; only completed runs link to proposals, and reviewer
domain commands remain the sole scientific-write authority.

## Runtime roles and shutdown

`deepref-server serve` runs HTTP only. `deepref-server worker` runs the bounded PostgreSQL job executor. `deepref-server all` runs both roles in one process with coordinated shutdown. Hosted deployments package the same `deepref-server` binary for API and worker targets; local Process Compose supervises the API, worker, and web processes.

Crate boundaries follow dependency direction: `deepref-domain` owns pure invariants, `deepref-application` owns use-case commands and the minimal `JobQueue` port, `deepref-postgres` owns SQLx migrations and adapters, and `deepref-http-api` owns HTTP adapters and SQL-backed handlers. See [ADR 0002](adr/0002-layered-crate-boundaries.md).

## API and web degradation

The API exposes process liveness, PostgreSQL/schema readiness, durable worker-job status, and per-project graph metric freshness. Collections use bounded cursor pagination. The web app polls dependency status independently and keeps project, article metadata, ingestion, settings, and navigation available while queued work drains. Graph and recommendation routes read PostgreSQL directly and do not return an external-graph `503`.

The application has no user/tenant model. Cloudflare Access protects each hosted environment using configured GitHub organization membership, and all admitted users have equal application privileges.

## Hosted platform ownership

- OpenTofu roots under `infra/environments` own AWS infrastructure and global Cloudflare/GitHub policy plus initial Argo installations.
- The protected orphan `gitops` branch and Argo own hosted namespaces, workloads, External Secrets, policies, telemetry collectors, and `cloudflared`.
- Releases build three application images and one chart once from tested development source, sign/attest them, then copy exact OCI subjects through staging and production without rebuild.
- PostgreSQL migrations run only through `deepref-server migrate`; hosted Helm runs it as an Argo PreSync Job before Deployment changes. Normal `deepref-server serve` never migrates.

The repository contains deployable source, not proof of a deployed platform. Current apply/drill gaps are recorded in [production acceptance](acceptance/production-platform.md) and [operations](operations/README.md).

## Local development

```bash
mise exec -- just dev
```

This starts loopback-only PostgreSQL, applies migrations, then supervises web, API, and worker. Compose contains no application services and is not a deployment artifact. See [local development](local-development.md).

## Workspace dependency contract

Run `just architecture` (or `cargo xtask boundaries`) to validate all workspace
members using Cargo metadata. Each manifest declares its existing role through
`[package.metadata.deepref] layer`. Unclassified packages fail validation.
The tooling-only `xtask` package cannot be a dependency of production packages.

The checker covers normal, build, dev, optional, renamed, and platform-specific
dependencies. Domain/application external dependency restrictions and the removed
NATS guard remain enforced. The HTTP package may use the worker only as a dev
dependency for existing integration fixtures. SQL-backed HTTP handlers remain
permitted; persistence cannot depend on worker orchestration or HTTP adapters.

Just remains the public command interface, Mise owns tool versions, and xtask
owns the repository graph. Ordinary build, test, and service commands stay in Just.
