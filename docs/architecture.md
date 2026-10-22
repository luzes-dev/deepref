# DeepRef architecture

DeepRef maps article citation networks from seed DOIs. The SvelteKit SPA uses a same-origin `/api` contract through the web gateway. A Rust Axum API owns HTTP behavior; the Rust worker performs durable recursive ingestion and reconciliation; PostgreSQL is authoritative for state, jobs, graph facts, and metrics.

## Data and event planes

PostgreSQL owns projects, ingestions/items, works, project membership, canonical fetched-reference facts, citations, unresolved references, claims/leases, domain events, durable jobs, graph status, and metric snapshots. The v2 evidence workspace is layered alongside the citation-ingestion model: records, reports, studies, screening events/state, review events, documents, audit rows, and leased jobs are all PostgreSQL tables.

Authoritative state changes and their follow-up jobs are committed in one transaction. The worker claims jobs with `FOR UPDATE SKIP LOCKED`, owns a lease with expiry renewal, retries failures with persisted attempts, marks terminal failures dead, and reclaims expired work. Stable dedupe keys prevent duplicate logical jobs. Ingestion preserves cached/fetched facts, discovered children, citations, events, and completion in the same transaction boundary; reconciliation repairs already-committed rows without silently dropping work.

The generic transactional outbox (`outbox_events` with `outbox_receipts`) and
the provider integration inbox (`integration_inbox_events`, migration `0083`)
carry asynchronous publication and inbound WorkOS/Polar webhooks without a
broker. Outbox rows are inserted in the same transaction as the authoritative
change they describe and stay broker-independent: an in-process dispatcher in
the worker claims bounded batches with lease expiry renewal and delivers to the
typed handlers registered per message type and version, and a row is
`delivered` only when every expected consumer's receipt — written in the
consumer's own transaction, with its effect — has committed. Claiming or
leasing never delivers; per-consumer retries back off and quarantine after
their attempt budget, and quarantined rows are replayed explicitly with the
same stable event identity. The inbox verifies provider signatures before any
insert, deduplicates on `(provider, provider_event_id)`, acknowledges only
after the insert commits, and moves events through
`received -> processing -> processed | failed -> quarantined` with the same
retry and replay discipline. The domain-specific `ai_autonomy_outbox` is
deliberately kept separate from this generic outbox and is not consolidated
into it; a later migration would need its own explicit decision and
delivery/idempotency plan.

Provider handlers treat duplicate and out-of-order webhook events as normal:
they reload authoritative provider state or compare provider versions where
available. Provider-specific reconciliation hooks run from the worker's
existing periodic reconciler; WorkOS and Polar adapters register their hooks
when those integrations land.

Automation, compiled-review and workflow consumers read lifecycle status and
timestamps from `durable_runs`, `durable_steps` and `durable_attempts`.
Recipe, workflow graph and review provenance remain in their typed domain
detail tables. Audit exports preserve their public event names and
`automation_run_id` fields while projecting canonical lifecycle rows; AI-first
cohort evaluation and second-review retry backoff use the same canonical run
status. Review artifact acceptance and step completion share the live job-lease
fence and transaction, rather than maintaining a separate review attempt state.

There is one `durable_run_step` queue kind, dispatched by a typed handler
(`automation`, `workflow`, or `review`). Recipe steps enqueue their successor
transactionally rather than executing an entire recipe inside one queue claim.
The compiled-review planner replays already-completed typed artifacts and
schedules the next uncompleted node; only that node's live claim may execute
or accept an attempt. Reused evidence keeps its original accepted-attempt UUID,
while the current generation still records its own canonical attempt.

Migrations `0073` and `0074` preflight cross-engine UUID collisions, copy existing
run/step/attempt identities and review generations/reuse epochs, retarget jobs
without changing queue IDs, dedupe keys, attempts or live leases, and validate
the retargeted foreign keys. Queue and recipe-definition references retain the
deferred protection required for project deletion; accepted-attempt references
also enforce project identity. The migrations retire old lifecycle tables and
SQL entry points, not replace them with compatibility views. Terminal step jobs,
including exhausted expired leases, are reconciled to terminal canonical state
even when the handler crashes, without superseding an active successor claim.
Review scheduling can then create a new generation instead of reusing a
permanently running manifest.

`RuntimeConfig.workflow.niche_nodes_enabled` defaults to false through
`DEEPREF_WORKFLOW_NICHE_NODES`. Slack, arbitrary HTTP, webhook and inbound-email
nodes remain in the catalog with `available: false` so saved graphs render
unchanged, but the palette, API authoring/activation/ingress and queued worker
execution deny their use. Authoring, publishing and enabling recheck the effective
locked graph inside their persistence transactions. Outgoing `integration.email`
remains enabled. API
and worker composition install the same immutable application dispatch policy;
in-process configuration disagreement fails startup. When enabled, the existing
origin-bound credentials, signed hooks and token-before-rate-limit checks remain
authoritative.


The durable queue port carries the pure `deepref-domain::JobKind`, not an
arbitrary string. Producers and exhaustive worker dispatch share the same closed
set; SQL adapters preserve existing spellings, including `run_deduplication`.
Unknown persisted kinds fail with the job identity at row decoding and remain
stored for diagnosis/recovery; they are never reinterpreted as another task.
Existing `recompute_prisma` jobs retain their acknowledged compatibility
behavior because PRISMA is computed live.

Persisted lifecycle values are decoded into the owning pure domain, review or
AI enum before application decisions. This includes screening/document,
ingestion/acquisition, assistant plan/run, AI proposal, compiled-review run,
attempt and calibration, workflow and graph projection lifecycle states. Invalid stored values
propagate as persistence errors rather than fallback states. SQLx remains in
adapters; DTO conversion preserves the existing HTTP string spellings.

Graph reads use canonical UUIDs from `reports`, `project_reports`, and `citations`. Reports without DOI identifiers remain graph nodes. Responses are deterministic and bounded; edges are UUID-to-UUID and never depend on DOI mapping. Project metrics use the same degree/rank semantics as the legacy fixture and are recomputed after imports and later ingestions. The PostgreSQL graph status endpoint reports metric/projection freshness for compatibility with existing clients; it is not an external graph service.

The v2 screening command writes its event, state projection, and project lifecycle status in one transaction. PRISMA is a live deterministic projection of canonical relational state; no recomputation job or snapshot is authoritative. The command uses optimistic revisions and returns `409 screening_revision_conflict` when another browser, worker, or automation changes the same report.

The screening pilot lives in `deepref_app::screening`, in `crates/app`.
This is the renamed Postgres-backed application crate, not a facade over a
second backend crate. The module owns command SQL, queue/history readmodels,
blind-audit assistant reads and plan snapshots, and assistant activity
provenance. HTTP review routes, assistant confirmations, workers and autonomy
cross the same screening seam. Pure transition and blind-audit preconditions
live in `deepref-domain`; authoritative membership reads, row locks, revision
CAS, published-protocol checks and event writes remain in the app transaction.
Other contexts temporarily retain their existing app-root interfaces.
`cargo xtask boundaries` rejects SQLx and direct query entry points in migrated
review and assistant-plan transports; unmigrated contexts and dev-test targets
are intentionally outside that source rule.

### Acceptance status

Handoff 04a is foundation-only until 02/08 wire real WorkOS/Polar verifiers,
handlers, and reconciliation. Acceptance still needs provider fixtures for real
signatures, out-of-order projection updates, missed-event reconciliation, and
commit-before-ack behavior. Production registries are empty; metrics recompute
continues to enqueue jobs directly. Dispatcher effects are synthetic test
fixtures until the production pilot in 04b.

Heartbeat ownership checks are polled before ready handler work where possible.
Database effects must commit with the generation-fenced receipt or inbox
completion; an inbox preflight check takes no row lock, and completion is the
final write before commit so long effect transactions do not block renewal.
Cancellation cannot retract an external request or COMMIT already submitted.
External effects need downstream idempotency keys (outbox event ID + consumer,
or inbox provider + provider event ID), and downstream fencing where supported.

Inbox receipt rejects missing, empty, or whitespace-padded `event_type` with a
typed error. Provider adapters must supply the verified event's explicit type;
04a does not accept a provider contract that omits it.

Each due outbox consumer reserves one attempt per claim generation. Failure
bookkeeping and abandoned-claim recovery settle that reservation without
charging it again; consumers in backoff reserve nothing. Unsupported frozen
consumers are parked for five minutes with a diagnostic, and claims exclude
incompatible sets even beyond the bounded parking batch.

Inbox receipt accepts bodies up to 256 KiB. Processed payloads expire after
30 days; the explicit `purge_processed_inbox_payloads` app operation replaces
expired payloads with NULL while retaining `(provider, provider_event_id)` and
processing evidence for deduplication. No purge scheduler is installed.

## Object storage and blob inventory

PostgreSQL keeps metadata and provenance; object storage keeps bytes. All object
access goes through `deepref_documents::DocumentStore`, which wraps the
`object_store` crate. Development uses the filesystem backend
(`DOCUMENT_STORAGE_BACKEND=local`, allowed only when `APP_ENV=development`); production
configures an S3-compatible bucket with an HTTPS endpoint. Domain and
application code never see a provider type. Object keys are built by
`ObjectKey` from stable identifiers only, never from a user-provided filename:
`documents/{document_id}` for uploaded PDFs and
`projects/{project_id}/workflow-runs/{run_id}/files/{file_id}` for workflow
export files. Authorization is checked before a key is derived or read; the key
layout is not an access boundary. The workspace prefix from the SaaS tenancy
program is added with that gate.

Inventory of binary and large-text columns in `crates/app/migrations/`:

| Column | Holds | Status |
| ------ | ----- | ------ |
| `workflow_files.content bytea` (0028, up to 16 MiB) | Workflow export files (RIS, CSV, ...) | Moving to object storage, see below |
| `document_blocks.text` (0006) | Parsed text of one PDF block, indexed by the generated `search_vector` for full-text search | Stays: it is searched in place and each value is a single block, not a file body |
| `documents` rows (0006, 0013) | Metadata and an object key for the PDF; the PDF bytes are already in object storage | No change |
| `document_references.raw`, `fetched_unresolved_reference_facts.raw_unstructured`, `unresolved_references.raw_unstructured` | One citation string each | Short text, stays |
| `jsonb` payload and `raw` columns (`records.raw`, `reports.raw`, `works.raw`, dead-letter `payload`, `trigger_data`, review artifact `payload`) | Normalized or provider JSON | Relational data, out of scope |
| `assistant_messages.content` (0023), `notifications.body` (0024, at most 500 characters) | Chat and notification text | Stays |

`workflow_files.content` is the only column holding file bodies. It moves in
stages. Migration `0084` adds nullable `storage_key`, `content_sha256` and
`byte_size` (all null or all set) and keeps `content bytea NOT NULL`.

- **Dual write.** `create_workflow_file` writes the object first, then inserts
  the row with both `content` and the reference. If the insert fails the object
  is removed; a crash between the two leaves only an unreferenced object, never
  a row pointing at nothing.
- **Read.** `get_workflow_file` prefers the object and checks its size and
  SHA-256 against the row. A null key, a missing object, an unreadable object
  or a mismatch is logged and served from `content`; the result reports which
  case applied (`WorkflowFileSource`).
- **Backfill.** The durable job kind `backfill_workflow_files` is scheduled
  per project by the worker reconciler for rows without a key. It uploads
  `content` under the row's deterministic key, reads the object back, compares
  its SHA-256 with `content`, and only then sets the reference. It is idempotent
  and resumable: rows are selected by their missing reference, so a run that
  died resumes with what is left, and an object left by a dead run is
  overwritten and verified again.

Dropping `NOT NULL` on `content`, switching writers to object-only, and dropping
the column follow in later releases once old binaries have drained. Cleanup of
objects for deleted projects, replaced files and failed writes through durable
jobs is not implemented yet.

Assistant SSE (`crates/http-api/src/routes/assistant/run_events.rs`) replays
persisted run events from PostgreSQL by `after_seq`, so a stream can reconnect
to any API replica and needs no pub/sub.

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

The default local fixture is the pinned `pgvector/pgvector:0.8.7-pg17-bookworm` image.
This is disposable migration/integration tooling. The local acceptance evidence is
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

`deepref-server serve` runs HTTP only. `deepref-server worker` runs the bounded PostgreSQL job executor. `deepref-server all` runs both roles in one process with coordinated shutdown. Deployments package the same `deepref-server` binary for API and worker targets; local Process Compose supervises the API, worker, and web processes.

Crate boundaries follow dependency direction: `deepref-domain` owns pure invariants, ingestion/fetch states and work/reference models, event envelopes and payloads (`events`), and in-memory graph models/algorithms (`graph`). `deepref-application` owns use-case commands and the durable-work ports (`DurableExecutionStore`, `WorkTransport`); `deepref-review` retains `ReviewScheduler`. `deepref-integrations` owns the OpenAlex, Crossref and DataCite metadata adapters, PubMed, import parsers, Unpaywall and GROBID; `deepref-app` owns SQLx migrations and adapters; `deepref-http-api` owns HTTP adapters and SQL-backed handlers. See [ADR 0002](adr/0002-layered-crate-boundaries.md).

Runtime environment access belongs to `deepref-config`. Its typed
`from_source` constructors support deterministic injected configuration tests;
composition roots load the startup snapshot and pass typed values to API,
worker and infrastructure adapters. This covers worker execution limits,
document storage/PDF/GROBID, SMTP, HTTP policy, AI routing/provider settings and
Crossref enrichment. Build/toolchain and test-fixture environment inputs are
distinct from production runtime configuration.

`RuntimeConfig.workflow.niche_nodes_enabled` is the shared API/worker setting
for `DEEPREF_WORKFLOW_NICHE_NODES`, defaulting to false. Runtime configuration
defines that value; workflow feature admission and behavior belong to the
workflow context, not configuration loading.

## Durable jobs: execution store, work transport and workload pools

Durable job execution is split into two application ports
(`crates/application/src/jobs.rs`). `DurableExecutionStore` owns persisted
lifecycle semantics: enqueue, claim/fence, lease renewal, attempts, terminal
transitions, retry due times and recovery. `WorkTransport` owns delivery
mechanics: waking eligible work and handing it to the worker for a workload
pool. The only transport today is the PostgreSQL claim path
(`PostgresClaimTransport` over `PostgresDurableExecutionStore`,
`crates/app/src/jobs.rs`): delivery means claiming, and the claim remains the
authority. No provider SDK or SQLx type appears in the ports.

`services/worker/src/delivery.rs` keeps a pure `delivery_matrix` helper for the
05b broker response contract. The 05a PostgreSQL claim query checks queued
state, due time, attempt budget and workload class; it does not inspect whether
an owning run was cancelled or superseded. Runtime dispatch guards load the
owning run and skip terminal work before invoking its handler. Broker
ACK/NAK/term semantics belong to 05b, when a broker transport is introduced;
the 05a claim path has no broker response to send.

Business attempts increment when a claim is accepted and are refunded when an
unstarted claim is released. The independent monotonic `claim_seq` advances on
every claim and fences renew, complete, fail and release, even when a released
claim is reclaimed by the same worker owner. Failure retries persist their due
time in `available_at`, and expired-lease recovery reuses the attempt the
crashed claim already spent.

Every job row carries a typed scope — `project`, `workspace` or `platform`
(`jobs.scope`, with `project_id` / `workspace_id` as the matching tenant
references and a consistency CHECK). Workspace- and platform-scoped work runs
with no project row; claiming filters on workload class, never on
`project_id IS NOT NULL`, and no job fabricates a project.

Every job kind maps to exactly one workload class, derived from the kind at
enqueue (`JobKind::workload_class`, stored in `jobs.workload_class`) and never
chosen by the caller. This table is the normative mapping:

| Job kind               | Workload class | Bound / isolation rationale                              |
| ---------------------- | -------------- | -------------------------------------------------------- |
| `assistant_agent_run`  | `ai`           | model calls, bounded by provider and usage limits          |
| `ai_usage_reconcile`  | `ai`           | workspace-scoped recovery of fenced usage reservations    |
| `ai_usage_export_replay` | `ai`        | bounded replay emitting usage fact export events          |
| `retrieve_document`    | `documents`    | document pipeline: full-text retrieval feeding parsing     |
| `parse_document`       | `documents`    | CPU/memory-bound PDF parsing                              |
| `run_deduplication`    | `documents`    | CPU/memory-bound deduplication of imported record corpora |
| `durable_run_step`     | `workflow`     | durable automation, workflow and review step dispatch      |
| `backfill_workflow_files` | `workflow`     | workflow export file copy to object storage                |
| `work_fetch_requested` | `integration`  | external metadata provider traffic, rate-limit bounded     |
| `pmid_import`          | `integration`  | external PubMed provider traffic, rate-limit bounded       |
| `recompute_metrics`    | `projection`   | derived read-model recomputes                             |
| `recompute_prisma`     | `projection`   | derived read-model recomputes (compatibility)             |

The class set contains only classes real kinds need: there is no `review` class
today because review work rides `durable_run_step` (handler kind is a dispatch
detail, not a queue kind). Add a class only when a job kind needs it.

`deepref-server worker --pool <class>[,<class>]` runs one process for the
selected pools; without `--pool` a worker serves every class, preserving the
single-process behavior. Several replicas of one pool share the same durable
consumer. Concurrency is set per pool (`WORKER_CONCURRENCY_<CLASS>`, defaulting
to `WORKER_CONCURRENCY`), so a saturated pool neither drains nor starves
another, and each tick claims at most `min(free pool capacity,
WORKER_CLAIM_BATCH_MAX)` jobs in one statement — never in an unbounded inner
loop. Idle claim ticks back off instead of polling hot.

## API and web degradation

The API exposes process liveness, PostgreSQL/schema readiness, durable worker-job status, and per-project graph metric freshness. Collections use bounded cursor pagination. The web app polls dependency status independently and keeps project, article metadata, ingestion, settings, and navigation available while queued work drains. Graph and recommendation routes read PostgreSQL directly and do not return an external-graph `503`.

The SPA uses the live workflow builder, not the retired renderer-neutral
workflow prototype or assistant preview fixtures. Scaffold demo routes and
Vitest examples, unused web typewriter/hook re-exports, and the unreachable
deduplication page are removed. The `/deduplication` redirect still sends
existing links to `/discovery/duplicates`. Sass/Rete preprocessing is removed
because no live SCSS source remains. The shared UI layout barrel exposes only
live layout composition, while live chat cards and BuilderCanvas retain their
explicit entry points. Generated operations are unchanged until merged API
regeneration.

The application has no user/tenant model.

## Local development

```bash
mise exec -- just dev
```

This starts loopback-only PostgreSQL, applies migrations, then supervises web, API, and worker. See [local development](local-development.md).

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
The checker also rejects direct runtime environment reads in the migrated
server, worker, HTTP, AI, document, integration and telemetry source contexts,
and the migrated workflow persistence boundary. Configuration, build scripts,
tooling and test-fixture inputs remain separate. This is not a blanket ban on
SQLx in HTTP or worker contexts that have not migrated their persistence seams.

The workspace contains 13 members. The former `core`, `events` and `graph`
packages are domain modules, not compatibility crates; the former `crossref`,
`providers` and `grobid` packages form `integrations`. GROBID keeps its HTTP/TEI
behavior. Unused catalogue
models and graph-upsert helpers are removed. Zero-implementation
`DocumentRepository`, `OcrEngine` and `FullTextResolver` ports are removed;
live document storage, queue and review scheduling contracts are retained.
Telemetry remains a separate runtime adapter in this low-conflict cutover.
Moving SQL-backed handlers into use cases and forbidding SQLx at the HTTP/worker
boundary is a later migration, not a claim made by the current rules.

DOI metadata runs through a provider chain. The `WorkProvider` port in
`deepref-application` takes one DOI and returns its work with references, or a
typed error. `ProviderRouter` in the same crate walks the chain for each DOI.
The deployment chain from `DEEPREF_METADATA_PROVIDERS` is the allowlist: a run
keeps only the providers the deployment still enables, in stored order.
`deepref-integrations` implements the port for OpenAlex, Crossref and DataCite.
The first provider that answers wins, and its metadata becomes the work. After
every OpenAlex answer the router asks Crossref for its reference list when
Crossref is enabled in the deployment, whatever the run's own chain says, and
merges it in: a DOI both lists name keeps OpenAlex's position and takes
Crossref's bibliography, and entries without a DOI follow as unresolved
references. A transient Crossref failure retries the fetch instead of caching a
short list, and a fatal one is a degraded success. A reference failure after the
metadata answered never selects another metadata provider. A fatal failure of one
provider is recorded and the chain moves on. `works.metadata_provider` records
the provider that answered. `works.citation_provider` lists the providers whose
references were kept, in contribution order, and is empty when none were. The raw
payload keeps every attempt with its purpose and request count, and
`deepref_reference_status` says whether the reference plan completed or was
degraded. Abstract enrichment stays best-effort and takes a permit for every
request. Publication alerts and PMID imports keep their own providers.

Just remains the public command interface, Mise owns tool versions, and xtask
owns the repository graph. Ordinary build, test, and service commands stay in Just.

## Workspace tenancy: project backfill (handoff 03a)

Every project belongs to one workspace. The staged rollout is expand-only until
the final contract step:

1. `deepref-server migrate` (once, before any new replica starts) creates the
   workspace tables, the fixed migration workspace, the nullable
   `projects.workspace_id`, and a BEFORE INSERT trigger that assigns the
   migration workspace to any project inserted without one (previous-release
   binaries). It also runs one bounded backfill batch and grants existing users
   ownership of the migration workspace. The `projects (workspace_id)` index is
   built `CONCURRENTLY` by its own `-- no-transaction` migration.
2. Operator step, after the roll: `deepref-server backfill-project-workspaces`.
   It moves projects that still lack a workspace in batches of 500, one
   transaction per batch, then validates (total, unassigned, orphaned) in a
   single statement. The same command marks existing users who hold an active
   membership as onboarded, in bounded batches (sign-in does the same for any
   it has not reached). It is resumable and safe to rerun; a nonzero unassigned or
   orphaned count fails the command and changes nothing.
3. Only after the command reports a clean validation may a later
   `-- phase: contract` migration add `NOT NULL` and drop the trigger. That
   migration needs a `requires-release-out-of-service` line.

Onboarding is tracked by `users.onboarded_at`, not by the user insert, so a user
the previous release provisioned (no memberships) is onboarded by their next
sign-in. Invitations bind only to the address the provider verified
(`users.verified_email`, which must equal the current `primary_email`). WorkOS reconciliation is serialized per Organization
and rejects any snapshot that began before `workspaces.workos_reconciled_through`.

## Migration recovery: the denial-window index (handoff 03b)

Migration 0100 builds `admin_audit_denial_window_idx` with
`CREATE UNIQUE INDEX CONCURRENTLY`, outside a transaction. If it is interrupted
(process killed, lock timeout, a duplicate key found while building), PostgreSQL
leaves an INVALID index of that name and the next run of 0100 fails with
"relation already exists". Recover by hand, then rerun the migrations:

```sql
DROP INDEX CONCURRENTLY IF EXISTS admin_audit_denial_window_idx;
```

Migration 0101 asserts the index is valid, ready and unique and refuses to
continue otherwise, so a skipped or half-built index can never be accepted. The
two migrations are unreleased; once released they are immutable and any repair
is a new migration.
