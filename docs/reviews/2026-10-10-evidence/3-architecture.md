> Historical review. This document describes the pre-SaaS architecture and is not normative documentation.

# Reviewer 3: Architecture, layering, crate structure, simplification

Scope: crate layering, where business logic lives, shallow crates and speculative ports, competing patterns, large modules, duplication, and a zero-based target shape. Other reviewers already cover these topics, so this report only cross-references them:
- Data/jobs (1): legacy plus v2 dual write, `domain_events`, the plpgsql `*_legacy` chains, the `handle_job_with_*` wrappers, stringly SQL states.
- Frontend (4): dead `workflows/domain`, the typewriter files, `vitest-examples`, giant components.
- AI (5): the Rig gateway, the two wire stacks, the review DAG versus automations, embeddings.
- Testing (6): DB tests skipped in CI, Z.AI config.
- Product (7): workflow engine size, calibration bundles, the two import pipelines.

All line counts below exclude `tests/` directories and `*.spec`/`*.test` files. Inline `#[cfg(test)]` modules are included.

---

## 1. Mental model (as built)

### 1.1 Crate graph and size

| Crate | Layer (xtask) | Non-test LoC | Internal deps |
|---|---|---|---|
| `crates/app` | persistence | 26,703 (+17k tests) | application, ai, graph, domain, documents, events, review |
| `crates/http-api` | http | 18,773 (+9.9k) | 11 of the 15 library crates (everything except `grobid`, `telemetry`) |
| `crates/ai` | ai | 14,950 | domain |
| `crates/application` | application | 12,337 | domain |
| `crates/review` | review | 8,353 | ai, application, domain |
| `services/worker` | worker | 7,150 (+3k) | 12 crates |
| `crates/documents` | adapter | 3,984 | application, domain |
| `crates/domain` | domain | 2,116 | (none) |
| `crates/providers` | adapter | 1,593 | application, core, crossref, domain |
| `crates/crossref` | adapter | 918 | core, domain |
| `crates/grobid` | adapter | 603 | (none) |
| `crates/config` | configuration | 586 | (none) |
| `crates/events` | ? | 453 | (none) |
| `crates/graph` | graph | 441 | core |
| `crates/core` | core | 230 | domain |
| `apps/server` | composition | 225 | http-api, config, telemetry, worker |
| `crates/telemetry` | ? | 115 | config |
| `apps/web/src` | n/a | 43,880 | |
| `packages/ui/src` | n/a | 11,831 | |

The workspace has 18 members for about 100k lines of non-test Rust. `services/api/` is an empty, root-owned leftover directory with no tracked files.

### 1.2 Where business logic actually lives

| Place | What lives there | Evidence |
|---|---|---|
| `crates/domain` (2.1k) | Typed IDs, the screening state machine (`transition`, `undo_transition`, `lib.rs:180-236`), protocol validation, bibliography identifiers | Pure, no I/O |
| `crates/application` (12.3k) | **Pure** rules (appraisal/RoB2, PRISMA arithmetic, dedup scoring, the workflow planner, validation and catalog), plus 10 port traits | **0 `async fn`** in the whole crate. It holds no use cases. |
| `crates/app` (26.7k, 639 `sqlx::query` calls) | The **de facto application layer**: preconditions, locking, orchestration, cross-context reads, visibility policy | e.g. `screening.rs:140-800` |
| plpgsql in migrations | 38 functions and 24 triggers: dispatch, visibility, transitions, immutability | `dispatch_automation_trigger`, `ai_first_run_visible`, `ai_screening_proposal_withheld`, `enforce_automation_run_transition`, `recompute_project_metrics`, … |
| `crates/http-api` | Orchestration and SQL in handlers (61 queries in 16 files, 8 `begin()`), external HTTP calls | `routes/acquisitions.rs` (18 queries), `routes/documents.rs:975` (Unpaywall) |
| `services/worker` | Ingestion persistence (47 queries in `store.rs`), review execution, outbound HTTP, SMTP | `store.rs`, `processor.rs:902-1708`, `workflows/net.rs` |

So the code's stated architecture is hexagonal (domain, then application ports, then adapters), but in practice it is a transaction script. The crate called "application" is really "domain, part 2", and the crate called "postgres" is the application service layer. That gap explains most of the findings below.

---

## 2. What is good and should be preserved

- **Pure decision kernels exist and are well isolated**: the screening `transition`, the workflow run planner (`application/src/workflows/engine.rs`, idempotent and re-run after every node), PRISMA reconciliation, and RoB2 scoring. They are the right shape, and zero-based work should grow them.
- **Layer rules are machine-checked** (`cargo xtask boundaries` plus sentrux). The dependency direction domain → application → adapters holds: nothing points backwards.
- **Textual duplication is low.** jscpd over non-test Rust found 2.49% duplicated lines (2,497 of 100,448; 237 clones), mostly intra-file DTO/row repetition. The duplication problems are semantic (parallel models), not copy-paste.
- **One binary** (`apps/server`) composes API and worker. That is the right deployment shape for a single-tenant product of this size.
- **The AI ports are genuinely polymorphic.** `AiGateway` has 14 impls (cassettes, fakes, metered, routed), and `RemoteDocumentFetcher` and `DocumentParser` have real test doubles. Those seams pay for themselves.

---

## 3. Findings (ranked by leverage)

### A1. There is no use-case layer: business rules are spread across six places, and the `postgres` crate is the application layer in disguise

- **Current behavior.**
  - `deepref-application` has no async code and no orchestration (`git grep 'async fn' crates/application/src` returns nothing).
  - Every use case is written as a free function in `deepref-app` that takes `&PgPool` or a transaction and mixes precondition checks, locking, domain calls, and SQL. Example, screening a report:
    - The pure rule is in `domain::transition` (`domain/src/lib.rs:180`).
    - "Protocol must be published" is in `postgres/src/screening.rs:751` (`ensure_published_protocol`).
    - "Exclusion reason must belong to the project" is at `:770`.
    - The project-wide lock is at `:682`.
    - Event and state persistence are at `:520`.
  - Other rules live in plpgsql. Run-visibility policy (`ai_first_run_visible`, `ai_first_queue_visible`, `ai_screening_proposal_withheld`) and automation dispatch and transitions (`dispatch_automation_trigger`, `enforce_automation_run_transition`, `enforce_automation_step_transition`) are SQL functions and triggers. Data reviewer F12 and §4 cover the `*_legacy` rename chains.
  - HTTP handlers also orchestrate:
    - `routes/acquisitions.rs` runs 18 raw queries and owns `persist_doi_acquisition` (`:683`) and `enqueue_seed_jobs` (`:806`).
    - `routes/ingestions.rs:160` reaches into the sibling route module (`super::acquisitions::enqueue_seed_jobs`) to share logic between the two import pipelines.
    - `routes/documents.rs:975` constructs an Unpaywall client inside the handler.
  - The worker owns its own persistence: `services/worker/src/store.rs` (1,028 lines, 47 queries) writes works, citations, v2 reports, domain events, and run status outside the persistence crate.
- **Why it matters.**
  - A rule's location depends on who wrote it and when, not on what kind of rule it is. To change "who may see an AI proposal" you must read Rust in `postgres/ai_exposure.rs`, a plpgsql function, and the web fallback.
  - No rule above the pure kernels can be tested without Postgres. Reviewers 1 and 6 show that DB tests silently skip in CI, so most business rules are effectively untested in CI.
  - Three entry points (HTTP, worker jobs, assistant/agent tools via `postgres/agent_tools.rs` and `assistant_host.rs`) each re-compose the same operations. jscpd shows `http-api/routes/assistant/chat.rs:591-629` duplicated in `postgres/assistant_host.rs:300-338`.
  - The crate names mislead new contributors and agents. `AGENTS.md` and the architecture doc describe ports and adapters that are not used (see A2).
- **Evidence.**
  - `crates/application/src/lib.rs` contains only `pub mod` and `pub use`.
  - `postgres/src/lib.rs` re-exports hundreds of free functions.
  - SQL counts: postgres 639, http-api 61, worker 47.
  - 38 distinct plpgsql functions and 24 triggers across 46 migrations.
- **Proposed direction.** Stop pretending there are two layers. See §5 option B: make each bounded context (screening, protocol, acquisition/imports, documents, review/AI, workflows) one module that owns:
  - (a) pure rules, moved into `domain`;
  - (b) use-case functions that take `&mut Transaction` and contain its SQL;
  - (c) its read models.
  
  HTTP handlers, worker jobs, and assistant tools then call the same use-case function. Keep plpgsql only for **invariants that must hold against any writer**: immutability triggers such as `workflow_versions_immutable`, `reject_review_artifact_changes`, and `prevent_published_protocol_content_mutation`. Move **policy** (visibility, dispatch, transition tables) into Rust, where it can be unit-tested.
- **Verification.** After moving one context (screening is the smallest and best tested), `git grep sqlx -- crates/http-api/src/routes/review.rs` is empty. The HTTP and assistant paths call one function. Pure precondition tests run under `cargo nextest --lib` without `DATABASE_URL`.

### A2. Speculative ports: most `application` traits have zero or one implementation, and production code bypasses the ones that exist

- **Current behavior** (implementations counted with `git grep "impl … for"`):

| Trait (file) | Impls | Real use |
|---|---|---|
| `DocumentRepository` (`application/src/documents.rs:45`) | 0 | none |
| `OcrEngine` (`documents.rs:51`) | 0 | none |
| `FullTextResolver` (`acquisition.rs:115`) | 0 | none |
| `DocumentStore` (`documents.rs:36`) | 1 (`documents/src/lib.rs:488`) | `write_pdf` and `read_pdf` are never called. Callers use the concrete `DocumentStore::put`/`get` (a struct with the same name as the trait). |
| `JobQueue` (`jobs.rs:29`) | 1 (`postgres/jobs.rs:192`) | Used only in `crates/app/tests/jobs.rs`. Production calls the free functions `enqueue_job` and `claim_job` (`jobs.rs:30,69`). |
| `MetadataProvider`, `CitationProvider`, `SearchProvider` (`acquisition.rs:99-113`) | 1 each (`CrossrefProvider`, `StaticSearchProvider`) | Never called through the trait. The worker calls concrete `CrossrefProvider::fetch_work_with_references` (`processor.rs:160`, `workflows/nodes.rs:560`). `StaticSearchProvider` has no callers outside its crate. |
| `ReviewScheduler` (`review/src/types.rs:304`) | 2 | `MemoryReviewScheduler` (`review/src/memory.rs`) has no callers. Every production and test call constructs `PostgresReviewScheduler::new(pool)` concretely. |

- **Why it matters.** Each port multiplies the files a reader must open to follow a call: trait, adapter, concrete method, free function. Because callers bypass the traits, the ports document an architecture that does not exist, and they give agents false affordances to "implement the port". They also double the API surface (`put`/`get` versus `write_pdf`/`read_pdf`).
- **Proposed direction (deletion test applied).** Deleting `DocumentRepository`, `OcrEngine`, `FullTextResolver`, `MetadataProvider`, `CitationProvider`, `SearchProvider`, `StaticSearchProvider`, the `JobQueue` trait, `PostgresJobQueue`, the `DocumentStore` trait impl, and `MemoryReviewScheduler` plus the `ReviewScheduler` trait changes no production behavior. Complexity does not reappear elsewhere: callers already use the concrete types. Keep traits only where a second implementation exists today: `AiGateway`, `UsageLedger`, `RemoteDocumentFetcher`, `DocumentParser`, `AgentModelFactory`. Add a port later, when a second adapter actually arrives.
- **Verification.** `cargo check --workspace --all-targets` passes after removal. `cargo shear` may then drop dependencies (`futures`, `bytes`) from `application`.

### A3. Shallow and vestigial crates: 18 workspace members, of which 6 are under 650 lines and 3 are mostly re-exports or dead code

- **Current behavior.**
  - **`deepref-core` (230 lines).**
    - `doi.rs` is `pub use deepref_domain::{DoiError, normalize_doi};`, a pure re-export.
    - `ArticleSummary`, `Project`, and `Ingestion` have no users outside the crate.
    - What remains in use is the Crossref wire model (`Work`, `Reference`, `WorkWithReferences`, `FetchStatus`) and two status enums, used only by `crossref`, `providers`, `graph`, and the worker's `store.rs`.
    - Its existence forces a fourth "core" layer in `xtask/boundaries.rs:38`.
  - **`deepref-graph` (441 lines).** `compute_metrics` (petgraph) and the overlay types are used. `upsert.rs` (`build_graph_upsert`, `GraphUpsert`) has no callers outside the crate. `references.rs` is one hash function.
  - **`deepref-events` (453 lines).**
    - It is a NATS-era subject catalogue. `SUBJECT_WORK_FETCH_COMPLETED`, `SUBJECT_PROJECTION_COMPLETED`, `SUBJECT_METRICS_UPDATED`, `SUBJECT_REFERENCES_DISCOVERED`, and `WorkFetchCompleted` have 0 external uses.
    - There is no broker. `xtask` still special-cases a ban on `async-nats` (`boundaries.rs:190`).
    - The live parts are `EventEnvelope` and `DomainPayload` (used for the `domain_events` outbox the data reviewer found is write-only) and `DeadLetterRecord`.
  - **`deepref-telemetry` (115 lines)** is used once, by `apps/server`.
  - **`crossref` (918), `providers` (1,593), `grobid` (603)** are three adapter crates for external bibliographic services. `providers` mostly wraps `crossref`. A4 covers this.
  - **`deepref-config`** exists, but 32 `std::env::var` calls in 10 non-test files bypass it (A4).
- **Why it matters.**
  - Every crate adds a `Cargo.toml`, a layer entry in `xtask`, sentrux rules, a docs entry, and a compile unit.
  - The scheduled mutants workflow targets `deepref-core` by default (reviewer 6), so CI quality budget goes to a 230-line crate of wire structs.
  - Agents must learn a "core" versus "domain" distinction that no longer means anything.
- **Proposed direction.** Collapse the workspace to about 9 crates (§5):
  - Fold `core`'s live types into the integrations crate (they are Crossref wire types) and delete the rest.
  - Fold `graph` into the context that computes metrics (citation graph) and drop `upsert.rs`.
  - Reduce `events` to `EventEnvelope` plus the payloads still used and move it next to the outbox writer, or delete it with the write-only `domain_events` table (data reviewer F6).
  - Fold `telemetry` into `apps/server`.
  - Merge `crossref`, `providers`, and `grobid` into one `integrations` crate.
- **Verification.** `cargo xtask boundaries` and `sentrux check` pass with fewer layers, and clean-build `cargo build` time drops. Measure before and after with `cargo build --timings`.

### A4. External services have two or three clients each, and runtime configuration bypasses the config crate

- **Current behavior.**
  - **Crossref** has three clients:
    - `crates/crossref::CrossrefClient`;
    - `crates/providers::CrossrefProvider`, a wrapper that reads `DEEPREF_ABSTRACT_ENRICHMENT` from the environment inside `new()` (`providers/src/crossref.rs:17-19`);
    - a hand-built `reqwest` URL in `services/worker/src/workflows/pubmed.rs:68-75` (`search_crossref`, with its own query encoding and `select=` list).
  - **PubMed** has `providers/src/pubmed.rs::PubmedClient` plus worker wrappers. It is paced through a DB-backed limiter (`workflows/pubmed.rs:25`), separate from the ingestion limiter (`services/worker/src/limiter.rs`).
  - **Unpaywall** is constructed inside an HTTP handler (`http-api/src/routes/documents.rs:975`).
  - **SMTP** settings are read from the environment on every send (`worker/src/workflows/net.rs:151-195`, 8 `env::var` calls), not through `deepref-config`. `documents/src/parser.rs` makes 10 more env reads.
- **Why it matters.**
  - Mailto, retry, timeout, and rate-limit policy differs per call site. The Crossref polite-pool rules are honored by one client and not obviously by the workflow search.
  - Config errors surface at first use, not at startup. A misconfigured SMTP host is discovered when an automation fires.
  - The hosted fail-fast guards that reviewer 6 discusses (F10) cannot see these variables.
- **Proposed direction.**
  - Create one `integrations` crate with one typed client per service, built once at the composition root from `deepref-config` and passed in.
  - Move the workflow Crossref search onto `CrossrefClient`.
  - Add an `SmtpConfig` and a `PdfParserConfig` to `deepref-config` and validate them at startup.
- **Verification.** `git grep 'std::env::var' -- crates services | grep -v 'crates/config\|tests/'` returns nothing. `git grep 'api.crossref.org'` matches one file.

### A5. Three orchestration and run models, plus jobs, for "do steps durably"

- **Current behavior.**
  - (1) **Recipe automations:**
    - tables `automation_definitions`, `automation_runs`, `automation_step_runs` (migration 0019);
    - `application/src/automations.rs` (978 lines);
    - plpgsql dispatch and transition triggers;
    - `postgres/automations.rs` (891 lines).
  - (2) **User workflow engine:**
    - table `workflow_runs` (0028);
    - `application/src/workflows/*` (about 6k lines);
    - `postgres/workflows.rs` (2,390 lines);
    - `worker/workflows/nodes.rs` (1,814 lines).
  - (3) **Compiled review DAG:** `crates/review` (8.3k lines), executed by `worker/processor.rs:902-1708`.
  - (4) The durable `jobs` queue, which (1) through (3) each enqueue onto, under different job kinds.
  - Workflow templates point back to built-in recipes (`application/src/workflows/templates.rs:72-120`), and review runs ride on automation runs (AI reviewer §1.1). So the three models are entangled, not independent.
- **Why it matters.**
  - Three state machines, three idempotency schemes (AI reviewer F1), three retry semantics, and three run inspectors in the UI.
  - Fixing job fencing (data reviewer F3) has to happen in three places.
  - This is the biggest source of code volume after persistence: about 20k lines (product reviewer).
- **Proposed direction.** Define one durable run primitive: a run row, step rows, and jobs as the executor. Steps get a typed `StepKind` enum with one handler each. Recipes become seeded workflow definitions. The review DAG becomes a step implementation (or the flat "AI job = function" of AI reviewer §5 A) instead of its own engine. Keep the pure planner from `application/workflows/engine.rs`: it is the best part. This is a large migration. Sequence it after A1 and after the product decision on how much of the workflow engine survives (product reviewer).
- **Verification.** One `runs` table family. `processor.rs` dispatches on an enum with three or fewer variants related to runs.

### A6. Stringly typed seams, and three hand-mirrored representations of each read model

- **Current behavior.**
  - Domain enums exist (`ScreeningStage`, `ScreeningDecision`, …), but `postgres` read models carry them as `String`: 28 `pub status/stage/state/decision/kind: String` fields.
  - `http-api` re-declares each as a DTO, field for field. Example: `postgres/src/screening.rs:102-124` (`ScreeningHistoryItem`) is mirrored by `http-api/src/routes/review.rs:145-167` (`ScreeningHistoryItemDto`), found by jscpd.
  - `http-api` defines 142 `*Dto` structs, and conversion goes through 53 `*_dto`/`into_dto` functions and 27 `map_*_error` functions.
  - The OpenAPI schema is generated from the DTOs, so enum values reach TypeScript as `string`. The web then hand-lists them: `apps/web/src/lib/features/automations/model.ts:229-251` duplicates the server's `ConditionOperator` and fields, and `BLOCK_ALIASES` duplicates catalog knowledge.
  - Job kinds are string literals matched in `processor.rs:777-816`. One kind, `"recompute_prisma"`, is acked as a no-op (`:808`). Payload structs are declared in whichever module handles them, and some kind constants live in `deepref-app`.
  - IDs: `project_id: Uuid` appears 289 times across postgres, http-api, and worker, against 154 uses of `ProjectId`.
  - The data reviewer (F12) covers the SQL-side stringly states. This finding covers the Rust and TypeScript type boundary.
- **Why it matters.**
  - Adding a status means editing the enum, the SQL CHECK, the row struct, the DTO, the TS list, and sometimes a plpgsql function. The compiler checks none of these links.
  - The web cannot exhaustively switch on server states.
  - The `"recompute_prisma"` no-op shows that dead job kinds go unnoticed.
- **Proposed direction.**
  - Have postgres row decoding return domain enums (`sqlx::Type` or `TryFrom<&str>` in one place).
  - Derive `ToSchema` on the domain and read-model types, so DTOs disappear where they are identical. Keep DTOs only where the wire shape deliberately differs.
  - Add a `JobKind` enum with associated payload types in one module. The dispatcher matches exhaustively, and unknown kinds dead-letter.
  - Make typed IDs mandatory in public signatures. A clippy `disallowed_types` lint or a sentrux rule on `pub fn …(project_id: Uuid` can enforce it.
- **Verification.** Enum values appear as string unions in `apps/web/src/lib/api/generated`, and `model.ts` imports them instead of re-listing them.

### A7. God modules: a few files own several unrelated responsibilities

| File | Lines | Responsibilities mixed |
|---|---|---|
| `crates/app/src/workflows.rs` | 2,390 | 44 public functions: workflow CRUD and versions (`:226`), webhook and email tokens and credential rotation (`:647-785`), secrets (`:785`), the run engine and node executions (`:842-1689`), the scheduler (`:1689`), and **reads of other contexts' tables** for node data: `screening_queue_items` (`:2097`), `report_details` (`:2121`), `study_of_report` (`:2164`), `find_exclusion_reason` (`:2223`) |
| `services/worker/src/processor.rs` | 1,923 | DOI fetch and ingestion, metrics, document retrieve and parse, GROBID enrichment, the automation-run driver, and the whole compiled-review executor (`execute_compiled_*`, `persist_review_node`, `final_review_node`, `:902-1708`) |
| `services/worker/src/workflows/nodes.rs` | 1,814 | Every workflow node: HTTP, email, Slack, PubMed and Crossref search, AI, and DB actions |
| `crates/http-api/src/routes/workflows.rs` | 1,767 | Workflow routes plus inbound webhooks and email |
| `crates/http-api/src/routes/assistant/chat.rs` | 1,297 | Chat transport, plus tool-host logic duplicated in `postgres/assistant_host.rs` |
| `crates/http-api/src/routes/mod.rs` | 855 | Registers 254 routes in one function |

- **Why it matters.** These files are where most merge conflicts and agent edit failures will concentrate. `workflows.rs` reading screening and report tables directly couples the workflow engine to every context's schema, so a schema change in screening can break automations with no compile-time link.
- **Proposed direction.**
  - Split along the responsibility rows above.
  - In particular, move the review executor out of `processor.rs` into `crates/review` (or the AI context), behind one `run_review(job)` function.
  - Have workflow nodes call the owning context's use-case function (A1) instead of private SQL.
- **Verification.** No non-generated, non-test Rust file is over 1,200 lines. `git grep 'FROM screening_states\|FROM reports' crates/app/src/workflows.rs` is empty.

### A8. The boundary checks do not encode the boundaries that matter

- **Current behavior.**
  - `xtask boundaries` allows `Http | Worker` to depend on everything except each other and composition (`tools/xtask/src/boundaries.rs:49`).
  - `.sentrux/rules.toml` constrains only `domain` and `application`.
  - Nothing forbids `sqlx` in `http-api` or in the worker, or prevents one route module from calling another (`ingestions.rs` → `acquisitions.rs`).
- **Why it matters.** The checks pass while the layering erodes exactly where it is eroding.
- **Proposed direction.** Once A1 moves SQL out:
  - forbid `sqlx` as a normal dependency of `http` and `worker`;
  - forbid `reqwest` outside `integrations`, `ai`, and `documents`;
  - add a sentrux rule against `crates/http-api/src/routes/*` importing sibling route modules.
- **Verification.** `cargo xtask boundaries` fails on a deliberately added `sqlx::query` in a handler.

### A9. Smaller structural items

- **The worker's job entry point has 6 wrapper variants** (`processor.rs:311-383` plus `_inner` at `:764`). Tests bind to two of them. Covered by the data reviewer (§4); listed here for completeness.
- **Two import pipelines share code through route modules** (`ingestions.rs:52,160` call into `acquisitions`). The product and data reviewers cover consolidation. Architecturally, the merged pipeline should be one use-case module, not a route.
- **The retired Z.AI provider is still threaded through** `config/src/ai.rs` (44 refs), `ai/src/openai_compat.rs` (`ProviderDialect::Zai`), and `ai/src/pricing.rs`. Reviewer 6 covers the config side. Deleting the dialect removes a branch from the one provider adapter.
- **Dead web helpers** (`lib/typewriter.svelte.ts`, which duplicates `packages/ui/src/internal/actions/typewriter.svelte.ts`, plus re-export stubs and `vitest-examples`). Covered by the frontend reviewer.
- **`AGENTS.md` describes a `packages/core` that does not exist** and links to `/home/luzes/.t3/worktrees/...` paths (7 links). The `packages/core` mention is doc drift; reviewer 6 F14 covers agent-guidance drift.
- **`services/api/`** is an empty, root-owned directory and should be deleted.

---

## 4. Deletion candidates (architecture-specific)

| Candidate | Deletion test | Notes |
|---|---|---|
| `application` traits `DocumentRepository`, `OcrEngine`, `FullTextResolver` | Zero impls, zero callers | A2 |
| `MetadataProvider`, `CitationProvider`, `SearchProvider` traits and `StaticSearchProvider` | Never called through the trait. `StaticSearchProvider` has no external callers. | A2 |
| `JobQueue` trait and `PostgresJobQueue` | Used only in `postgres/tests/jobs.rs`. Production uses free functions. | Rewrite 2 tests against `enqueue_job`/`claim_job` |
| `application::DocumentStore` trait and its impl (`documents/src/lib.rs:488-530`) | `write_pdf`/`read_pdf` have no callers | A2 |
| `ReviewScheduler` trait and `MemoryReviewScheduler` (`review/src/memory.rs`) | No callers of the memory impl. Postgres is always called concretely. | A2 |
| `deepref-core`'s `ArticleSummary`, `Project`, `Ingestion`, and the `doi` re-export | 0 external uses | A3. Fold the rest into integrations. |
| `deepref-graph`'s `upsert.rs` | 0 external uses | A3 |
| Unused `deepref-events` subjects and `WorkFetchCompleted` | 0 external uses | A3. Possibly the whole crate, with `domain_events` (data F6). |
| `"recompute_prisma"` job kind | Acked as a no-op | A6. Check that no producer remains in SQL. |
| Worker `search_crossref` raw client | Duplicates `CrossrefClient` | A4 |
| `RigGateway` and `RigEmbeddingGateway` | No callers (the build fingerprint even records this) | AI reviewer F7 |
| Z.AI dialect and config | Retired provider | Reviewer 6 |

---

## 5. Zero-based redesign

Constraints of the product as it stands:
- single-tenant;
- one Postgres instance;
- one binary;
- roughly 15 bounded contexts, of which about 6 matter: protocol, imports/acquisition, screening, documents/full text, review AI, workflows;
- correctness and auditability matter more than throughput.

### Option A: Make the hexagon real

Move use cases into `application` as async services over port traits, and implement every port in `postgres`.

- **Pros:** matches the existing docs and the xtask layering, and use cases become testable with in-memory fakes.
- **Cons:** it doubles the code. About 300 SQL functions become a trait method plus an impl, and a transaction must thread through ports (unit-of-work abstractions). Fakes drift from Postgres semantics such as locking, constraints, and SERIALIZABLE. That fight is not worth it when the database is the source of truth and holds many invariants. This option repeats A2 at scale.

### Option B (recommended): Vertical context modules over Postgres, with a pure domain core

```
crates/
  domain/        pure: ids, enums, state machines, rules (today's domain + application's pure parts:
                 PRISMA, appraisal/RoB2, dedup scoring, workflow planner, validation)
  app/           one module per context: screening/, protocol/, imports/, documents/, review/, workflows/
                 each = use-case fns taking &mut PgTx + its SQL + its read models (returns domain types)
  ai/            gateway, prompts, metering (as today, minus Rig dead code and Z.AI)
  integrations/  crossref, pubmed, unpaywall, openalex/europepmc, grobid, smtp: typed clients built from config
  http/          axum routes: parse, call app::<context>::<use_case>, serialize; no sqlx
  worker/        job loop + exhaustive JobKind dispatch -> app use cases; no sqlx
  config/
apps/server      composition root, telemetry
tools/xtask
```

- **Rules.**
  - Only `app` depends on `sqlx`.
  - Contexts call each other's public use-case and read functions, never each other's tables. Enforce this with sentrux path rules per `app/<context>/`.
  - plpgsql is limited to invariant triggers (immutability, published-protocol locks). Policy lives in `domain` and is unit-tested.
- **Testing.** Pure rules are tested in `domain` without a database. Each `app` context has a DB test suite (which must actually run in CI: data F1, testing F1).
- **Why this fits.** It is what the code already does in `postgres`, renamed and given borders. Most of the migration moves code rather than rewriting it, and it deletes about 8 crates of indirection.
- **Cost:** medium. It can be done one context at a time behind unchanged HTTP contracts. OpenAPI snapshots (`pnpm run generate:api:check`) guard against regressions.

### Option C: Collapse the crate graph only

Keep code where it is, and merge `core`, `events`, `graph`, `telemetry`, `crossref`, `providers`, and `grobid`; delete the speculative ports.

- **Pros:** cheap (days), with an immediate cognitive and compile win.
- **Cons:** leaves A1 (rules in six places) and A7 (god modules) untouched.

### Suggested sequence

1. **Option C first.** It is low-risk and makes everything after it smaller:
   - delete the A2 ports and the §4 dead code;
   - merge the adapter crates into `integrations`;
   - fold `core`, `graph`, and `telemetry`.
2. Add a `JobKind` enum and route all config through `deepref-config` (A4, A6).
3. Pilot Option B on **screening**: move its preconditions to `domain`, rename `postgres::screening` to `app::screening`, and make `routes/review.rs` and the assistant `screen_reports` tool call it. AI F2 also wants the assistant to go through the governed path. Tighten the xtask rules (A8) for that context.
4. Repeat for protocol, then imports. Imports carries the legacy/v2 dual-write decision from data F6, so do it with that migration.
5. Decide the workflow and automation scope (product reviewer). Then unify the run models (A5) inside `app::workflows` and `app::review`.

---

## 6. Open questions

1. Is the hexagonal architecture an intended target, for example because a second storage backend or an embedded or offline mode is planned? Or is it scaffolding left by earlier agent passes? The answer decides between options A and B.
2. Is `deepref-events` meant to back a future external bus? If not, it and `domain_events` can go together.
3. Which plpgsql functions do you consider invariants that must hold even under manual SQL, and which are just policy? Several (`ai_first_run_visible`, `ai_screening_proposal_withheld`) look like policy placed in SQL for atomicity.
4. Should workflow nodes be allowed to read any context's data directly, or only through a defined set of read APIs? This determines how much of `postgres/workflows.rs:2097-2250` survives.
