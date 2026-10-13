# DeepRef: whole-application review

**Date:** 2026-10-10. **Commit:** `b0ad219` (main, clean).

**Method:**
- Seven independent reviewers each took one lens: data/jobs, security, architecture, frontend/UX, the AI subsystem, testing/ops/DX, and product.
- I ran the baseline gates.
- I re-verified the strongest claims myself, including running the DB-backed suite against a throwaway PostgreSQL.

**Evidence:** The seven raw reports and every log are in [`2026-10-10-evidence/`](2026-10-10-evidence/).
- Finding IDs such as `data F2`, `sec S3`, `ai F1`, `fe F1`, `ops F1`, `arch A1` and `prod H1` point to those reports, which hold the full file:line evidence.
- Claims marked **[re-verified]** were checked by me against the code or by execution.

**Severity:** Findings use P0–P3, Opportunity, or Exploration. Severity is judged against the **actual deployment model**: a single-user, self-hosted instance with no authentication.

---

## A. Executive assessment

**What it is.** DeepRef is a self-hosted workspace for systematic reviews:
- import and deduplication;
- title/abstract and full-text screening;
- study grouping, appraisal and extraction;
- PRISMA;
- citation graph;
- automations, an assistant, and governed AI proposals.

Its distinctive bet is **AI help you can defend in a methods section**. That rests on four things:
- append-only decision events;
- proposals with criterion-level, block-anchored evidence;
- blinding with exposure provenance;
- immutable run manifests.

The stack is a Rust Axum API and worker sharing one binary, PostgreSQL as the only store and queue, and a SvelteKit SPA with a shadcn-based UI package. It has about 100k lines of non-test Rust and 56k lines of TS/Svelte.

**Strongest qualities**
- **Real domain rigor.** The screening aggregate has an advisory lock, a revision CAS (compare-and-set) and a 409 that returns the authoritative state. The protocol is versioned and diffed, PRISMA is a live projection with no snapshot that can drift, and RoB 2 uses the official algorithm.
- **Transactional discipline.** State, follow-up job and audit row are written in one transaction, with no outbox.
- **Pure decision kernels.** The screening transition, the workflow planner, PRISMA arithmetic and RoB 2 scoring are isolated and pure.
- **Unusually strong supply-chain and CI hygiene.** Actions are SHA-pinned under a policy check, binaries are checksummed, images are digest-pinned and non-root, cargo-deny and pnpm audit are clean, and there is an OpenAPI/Orval drift guard.
- **Honest documentation culture.** ADRs and runbooks state their own gaps.

**Largest risks**
1. **A verification illusion.** About 30k lines of DB-backed integration tests (persistence, queue fencing, screening, AI-first, migrations, worker) **never run in CI**. They return `Ok(())` when `DATABASE_URL` is unset, and CI does not set it for the test step. When I ran them against a fresh DB, 837 of 856 passed. The 19 failures are cross-test interference on a shared DB (all pass serially). So the suite is largely sound, but nothing currently protects it. **[re-verified]**
2. **Integrity claims rest on an identity nobody verifies.** There is no authentication. The actor is a client-supplied header taken from per-tab `sessionStorage`, and it defaults to `local-user` on most routes and `web-user` on deduplication. Blinding, "fresh auditor" checks, dual-reviewer statistics and audit attribution are all built on that string. Deployment defaults are also unsafe: the bind address is `0.0.0.0`, `APP_ENV` defaults to `local`, and images are published to GHCR. **[re-verified]**
3. **Several paths can silently damage scientific state:**
   - an AI-first sweep that undoes audited exclusions after a *transient* error (data F2, **[re-verified]**);
   - assistant plans that write up to 5,000 screening decisions as the human, bypassing every AI-governance control (ai F2 / sec S7, **[re-verified]**);
   - a keyboard re-entrancy race that records decisions on reports the reviewer never saw (fe F1, **[re-verified]**).
4. **Complexity sprawl that does not match product value:**
   - three durable run engines (recipe automations, workflow engine, review DAG) on top of a job queue;
   - a legacy/v2 ingestion dual-write that never contracted;
   - speculative ports, and 18 crates for 100k lines;
   - a roughly 20k-line general-purpose workflow engine (webhooks, inbound email, Slack, HTTP, secrets);
   - meanwhile, **dual human screening, the core systematic-review workflow, does not exist**.

**Architecture maturity: medium.**
- *Locally* sophisticated: the screening aggregate, AI manifest identity and job transactions are well built.
- *Globally* drifting:
  - the declared hexagonal layering is not the real one, since `application` has no use cases and the `postgres` crate is the application layer;
  - business rules live in six places;
  - old and new models coexist indefinitely.

**Product maturity: early-to-medium.**
- The core single-reviewer workflow is good and was iterated with a real walkthrough.
- Team features, search documentation, prioritisation and meta-analysis-ready export are missing.
- The product's main differentiator, defensible AI, never reaches the user as an artifact.

**The single highest-leverage change.**
- **Engineering:** make CI actually execute the DB suite with per-test isolation, and fail when the DB is absent. It costs days, and every other recommendation in this review depends on that safety net.
- **Product:** a trusted reviewer identity. It unlocks dual screening, credible AI-first audits and an honest methods package.

---

## B. System mental model

```text
 Browser (SvelteKit SPA, svelte-query, orval client)
   │  x-actor-kind / x-actor-id (self-asserted, sessionStorage)   ← no auth anywhere
   ▼
 Caddy (web image): /api/* → API, else SPA          Vite proxy in dev
   ▼
 deepref-server serve (Axum, ~254 routes, 166 OpenAPI ops)
   │  routes → deepref-postgres free fns (639 queries)  + 61 queries in handlers
   │          → plpgsql (38 functions, 24 triggers: dispatch, visibility, transitions)
   ▼
 PostgreSQL 17 + pgvector  ── single source of truth: state, events, jobs, AI audit
   ▲
 deepref-server worker
   claim loop (SKIP LOCKED, 1 claim / 100 ms tick, lease 60 s, renew lease/3)
   reconciler 30 s (lease recovery, missing ingestion work, AI-first sweep, second-review sweep)
   workflow scheduler 20 s
   job kinds: work_fetch_requested, recompute_metrics, run_deduplication, pmid_import,
              automation_run (→ compiled review DAG), assistant_agent_run, workflow_step,
              retrieve_document, parse_document (PDFium in-process), recompute_prisma (no-op)
   ▼ outbound
 Crossref / PubMed / Unpaywall / OpenAlex / Europe PMC / GROBID / OpenAI-compatible LLM (OpenCode Go)
 SMTP, Slack, arbitrary HTTP (workflow nodes), inbound webhooks + inbound email (capability tokens)
 Object store: local fs or S3 (PDFs)
```

**Two data planes coexist** (data §1):
- **Legacy DOI ingestion:** `ingestions`, `ingestion_items`, `works`, `project_works`, `legacy_citations`, `unresolved_references`, `processed_events`, `domain_events`.
- **v2 evidence workspace:** `records`, `reports`, `project_reports`, `citations`, `acquisition_runs`, `studies`, protocol, `screening_events` → `screening_state`, documents/blocks, extraction, appraisal, plus the AI tables.

Every ingested DOI writes both planes in one transaction.

**AI execution** (ai §1):
1. An HTTP request prepares a typed subject.
2. `deepref-review` compiles a checked-in definition into a manifest and hash.
3. A review run is scheduled *as an automation run*.
4. The worker executes the DAG node by node:
   - step attempts are keyed by fingerprint;
   - `AiTaskRunner` validates output by JSON parse, then JSON Schema, then semantic checks;
   - each call is written to `ai_runs`.
5. The result becomes an `ai_proposals` row.
6. A human decision is a CAS (compare-and-set) that feeds the normal domain command.

The assistant is a separate stack:
- A Rig agent runs in the worker.
- Its read tools run immediately.
- Its write tools become a plan, which the user confirms before it executes.

**Trust boundaries:**
- browser → API: **unauthenticated**;
- API → LLM provider: article text leaves the machine;
- untrusted input → worker: PDFs, RIS/BibTeX/CSV, Crossref and GROBID XML, inbound webhook and email payloads;
- model output → state: proposals, plans, and `Act` autonomy for extraction and fuzzy duplicates.

**State ownership:**
- PostgreSQL is authoritative for everything.
- The web client derives state from server queries, with URL state for screening position and filters.
- A few browser-only stores contradict that:
  - the reviewer id is in sessionStorage;
  - automation canvas notes are in localStorage (fe F17).

**Critical journeys traced:**
- a screening decision (`ScreeningFocus.svelte` → `POST …/screening` → `screening.rs:164-279`);
- an AI suggestion, from request through proposal to acceptance;
- an assistant plan confirmation (`plans.rs:631-728`);
- DOI ingestion (`store.rs` → `complete_item_and_claim`);
- AI-first sweep and demotion;
- job claim, renewal and recovery.

---

## C. What is already good (preserve)

| Keep | Why it works |
|---|---|
| Screening aggregate (`postgres/src/screening.rs:164-279`) | Lock, revision CAS, 409 carrying the authoritative snapshot, event plus projection in one transaction. This is the model the other mutable aggregates should copy. |
| PRISMA as a single live statement over canonical tables | No snapshot to drift, no recompute job. The design deletes a whole class of bug. |
| One binary with `serve`/`worker`/`all`/`migrate` roles, and explicit migration (ADR 0001) | The right topology for a single-tenant product. Boring and reversible. |
| PostgreSQL as the only store, including the queue | Keeps "state + job" in one transaction. Do not introduce a broker. |
| Pure kernels: `domain::transition`, `application/workflows/engine.rs` planner, PRISMA arithmetic, RoB 2 | Testable without I/O. A zero-based redesign should grow these. |
| AI output pipeline: parse → normalize → schema → semantic, errors never echo model output | Correct order, fail-closed, well tested (184 offline tests). |
| Proposal acceptance as a CAS that dispatches to the normal domain command | Keeps "AI never writes scientific state directly" true on this path. |
| Ingestion item fencing (`store.rs:398-432`) and automation-step fencing on `claimed_job_attempt` | Real fencing. Generalise it rather than reinvent it. |
| Supply-chain/CI hygiene and the codegen drift guard | Better than most projects of any size. |
| URL-addressable screening state, keyboard core, skip links, lazily loaded pdf.js and sigma | Good frontend fundamentals. |
| ADR and acceptance-doc honesty ("Gap, verified", "attestation, not authentication") | Makes this review possible. Keep the habit. |

---

## D. Top findings (ranked by impact × reach × leverage × confidence ÷ cost)

| # | Finding | Category | Sev | Conf | Evidence |
|---|---|---|---|---|---|
| 1 | DB-backed integration suite (~30k LOC) never runs in CI. Tests skip silently without `DATABASE_URL`, and the CI test step doesn't set it. In parallel the suite shows 19 interference failures, because the mutex is in-process only and tests claim global jobs. | Defect (verification) | **P1** | Very high **[re-verified: 837/856 parallel; 33/33 of the failing binaries pass serially]** | `ci.yml:579-581`, `crates/postgres/tests/jobs.rs:18-19,64-66`, `db-tests.log`, `db-tests-serial.log` |
| 2 | No authentication. Identity is a client header from per-tab sessionStorage (two different defaults), yet blinding, fresh-auditor checks, kappa and audit attribution depend on it. Unsafe deploy defaults: `0.0.0.0`, `APP_ENV=local`, published images. DNS rebinding against the local API is plausible (no Host check). | Defect / Architecture | **P1** (P0 if network-exposed) | Very high **[re-verified]** | `routes/actor.rs`, `deduplication.rs:335-337`, `apps/web/src/lib/api/reviewer.ts`, `http-api/src/config.rs:17-18`, `config/src/lib.rs:41`; sec S1/S2/S8 |
| 3 | The AI-first sweep turns *any* failure computing semantic identity into a committed demotion that **reopens finalized, human-audited exclusions** and suspends AI-first, every 30 s on every worker. This contradicts ADR 0005 §2.4. | Defect | **P1** | High **[re-verified: `.ok().is_some_and` + sweep includes `finalized`]** | `ai_first/routing.rs:35-61, 261-275`, `cohorts.rs:283-386`; data F2 |
| 4 | Assistant `screen_reports` plans apply up to 25×200 decisions as the confirming human. They bypass autonomy settings, calibration, blinding and exposure provenance. The revision is read at confirm time (defeating the CAS), the card shows only the model-written summary, and each report is its own transaction (partial plans). | Defect / Product | **P1** | Very high **[re-verified]** | `routes/assistant/plans.rs:631-728`, `AssistantPlanCard.svelte:94-103`; ai F2, sec S7 |
| 5 | Holding or quickly re-pressing I/E/M records decisions on reports never displayed: `isPending` is false across `await updateUrl`, and `event.repeat` is not checked. | Defect | **P1** (integrity of the highest-frequency action) | High **[re-verified]** | `ScreeningFocus.svelte:312-334, 461-475`; fe F1 |
| 6 | Job queue robustness: | Defect | P2 | High | `jobs.rs:12-28,59-66,119-183`, `worker/src/lib.rs:92,124-133,294`, `crates/postgres/src/workflows.rs:1338-1620`; data F3/F4, ops F2 |
| | • lease recovery ignores `max_attempts`, so a poison job loops forever; | | | | |
| | • a panicking job task stops the whole worker process; | | | | |
| | • the fencing token is per process, not per claim; | | | | |
| | • workflow steps have no fence; | | | | |
| | • a renewal hiccup cancels in-flight work. | | | | |
| 7 | A failed or blocked review run can never be retried. The idempotency key is a pure function of the manifest, and terminal runs are returned forever. | Defect | P2 | High | `review_runs.rs:182`, `0019:429-440`; ai F1 |
| 8 | Workflow HTTP node: stored secrets are keyed by node, not origin. A templated or edited URL can exfiltrate them. Templates are not context-encoded. Inbound hooks rely on bearer-in-URL tokens and an unbounded rate-limit map. | Defect (security) | P2 (P1 if hosted) | High **[re-verified keying]** | `crates/postgres/src/workflows.rs:276-394`, `validate.rs:220`, `conditions.rs:191`; sec S3/S9/S11 |
| 9 | Run-model sprawl: recipe automations, the workflow engine and the review DAG each have their own state machine, idempotency and retry semantics, entangled through the jobs table. About 20k LOC of workflow/automation, roughly 4× screening. | Architecture | P2 | High | arch A5, ai §5, prod P1 |
| 10 | Legacy and v2 ingestion dual-write with no contract phase. About 10 tables plus N events per DOI. `ingestions` and `acquisition_runs` drift (seen in dev data). Status is recomputed O(N) per item under a READ COMMITTED race. Metric recompute is uncoalesced, with 35% serialization failures. | Architecture / Defect | P2 | High | `store.rs:261-996`, `graph.rs:180-277`; data F5/F6/F7 |
| 11 | Operators are blind: | Defect (ops) | P2 | High | ops F3/F9 |
| | • no access log at the default level; | | | | |
| | • job failures are not logged; | | | | |
| | • the error-body `correlation_id` is a fresh random UUID that matches nothing; | | | | |
| | • no metrics; | | | | |
| | • readiness checks schema ≥ 10 while the code needs 58; | | | | |
| | • the worker has no health surface. | | | | |
| 12 | Error semantics: | Defect | P2 | High **[re-verified]** | `screening.rs:204-206,276`, `review.rs:664`, `error.rs:62-82`; data F11 |
| | • retryable DB errors inside screening become `400 Bad Request` carrying raw sqlx text; | | | | |
| | • there is no SQLSTATE classification (40001/40P01/23505 become 500 or 400 at random); | | | | |
| | • five hand-rolled SQLSTATE checks. | | | | |
| 13 | Frontend data correctness: | Defect | P2 | High | fe F2–F6 |
| | • document blocks are capped at 100, so citations and overlays vanish on longer papers; | | | | |
| | • AI decisions invalidate every project query but miss the screening queue, causing self-inflicted 409s; | | | | |
| | • a transient `getProject` error navigates into a different project; | | | | |
| | • counts and filters cover loaded pages only. | | | | |
| 14 | No use-case layer: rules are spread over domain, application, the postgres crate, plpgsql, handlers and `worker/store.rs`. Plus speculative ports, 18 crates, stringly typed seams and three hand-mirrored read-model representations. | Architecture | Opportunity / P3 | High | arch A1–A8 |
| 15 | The product cannot represent a review team (no dual human screening). Search documentation and per-source PRISMA are missing. There is no screening prioritisation. The core value (defensible AI) never becomes a user artifact. | Product | Opportunity (high) | High | prod §3, H1–H4, S1 |

---

## E. Correctness and reliability findings

Beyond D #1, #3–#7 and #10–#13:

- **Unknown-outcome LLM retries are unmetered (ai F5, P2).** The gateway retries timeouts 3× with a 180 s timeout each, the job retries 5×, and repair adds re-asks. One stuck node can make up to about 30 provider attempts. Only successes are metered. The budget check is check-then-act across workers. On the assistant path, subscription-limit and budget errors are classified as retryable.
- **Orphaned assistant runs (ai F4, P2).** Several `Terminate` paths never call `fail_run`, so the UI long-polls forever. `list_assistant_run_events(...).unwrap_or_default()` hides DB errors as "no events".
- **Autonomy is applied after commit, best effort (ai F13, P3).** A crash loses the second-reviewer opinion permanently. Combined with D#7, that record can never get one.
- **The candidate-audit / semantic-repair loop (ai F6, P2, product semantics).** When the auditor disagrees, the rationale is repaired *until the auditor agrees*, and the disagreement is hidden from the reviewer. Each title/abstract record costs ≥ 2 calls (up to 7). No evaluation shows this improves accuracy.
- **The semantic identity hashes a stub `schema.json`, not the derived schema (ai F8, P2).** For the five non-screening definitions, Rust struct or validator changes do not change the identity that calibration and AI-first rely on.
- **Unbounded citation depth (data F9 / sec S5, P2).** Only `< 0` is rejected. Combined with O(N²) status recompute and per-item full metric recompute, one request can pin the worker and the Crossref budget for days.
- **Cancellation updates one of two run tables (data F5).** It has no status guard or existence check: you can "cancel" a completed run, and any UUID returns 202.
- **`dedupe_key` is unique forever (data F8, P3).** Re-enqueueing after completion silently no-ops. Documents work around this by resetting terminal jobs, which erases their failure history. The `jobs` table is never pruned.
- **The project-row `FOR UPDATE` in eight paths (data F10, P3)** blocks FK `KEY SHARE` checks on every child-table insert for the project. Lock order is undocumented (advisory, project, cohort, table SHARE), so deadlocks are possible and surface as 500s.
- **Last-writer-wins on extraction, appraisal and studies (data F13, P3).** The screening 409 contract is not reused.
- **The API has no graceful-shutdown deadline (ops F12, P3).** SSE observers hold termination until SIGKILL.
- **PDFium runs in-process under a global mutex (sec S12 / ops F2).** Time limits are checked only between pages, and there is no timeout around the blocking task. A hostile PDF can wedge parsing or crash the worker, and with D#6 that becomes a fleet-wide crash loop.

**Partial-failure map** (where state can become confused):

| Boundary | Failure | Result |
|---|---|---|
| Review run terminal failure | outage | permanently stuck (D#7) |
| Proposal finalized → autonomy apply | crash | lost opinion |
| Second-review claim tx → schedule tx | crash | permanent attempt with no run (data F14) |
| Assistant plan confirm | mid-loop error | partially applied plan with no rollback |
| AI-first identity read on a 2nd pool connection inside a locking tx | pool exhaustion | destructive demotion (D#3) |
| Ingestion last-two-items race | concurrent commit | run stuck `running` |

---

## F. Security and privacy review

**Threat model summary**

| Area | Detail |
|---|---|
| Assets | Unpublished review data, screening decisions and their *provenance* (scientific integrity), the AI provider key and budget, workflow secrets (webhook/SMTP/Slack tokens), uploaded PDFs |
| Actors | the local user; any process or web page reaching the API (DNS rebinding, local network); anyone if exposed; authors of imported articles and PDFs (prompt injection); inbound webhook/email senders |
| Boundaries | browser↔API (none), API↔LLM (data egress), untrusted files and payloads → worker, model output → state |

**Authentication and authorization.** None (D#2).
- Any reachable client can read everything, delete projects, read webhook secrets, run workflows, raise budgets to $100k, and spend the AI key (sec S1/S4).
- Clients can claim `x-actor-kind: automation|system`, forging AI or automation provenance (S8).
- Blinding and the "fresh auditor" check are defeated by opening a new tab.
- **Severity is contextual.** Acceptable for a loopback-only single user *if enforced*. It is not enforced: the bind default is `0.0.0.0`, there is no Host allow-list, `APP_ENV` defaults to `local`, and images are published.

**Input security:**
- *Already good:* CSV exports neutralise formulas. Assistant markdown is rendered safely, with no raw HTML and a scheme allow-list. The remote PDF fetcher and workflow HTTP client pin resolved IPs and cap redirects and size.
- *Gaps:*
  - two divergent SSRF block-lists that miss NAT64 and 6to4, and the workflow client honors `HTTP(S)_PROXY`, bypassing pinning (S13);
  - import parsers run synchronously in async handlers with no fuzz targets (S14);
  - templates are not context-encoded, so a quote can break JSON bodies (S11);
  - email recipients can come from templates with no cap, a possible mail relay (S10, inferred).

**Secrets:**
- No secrets in tracked files or history (`.env` and `.env.local` are ignored and untracked).
- Workflow secrets are stored **plaintext** and bound to (workflow, node, key) rather than to an origin (D#8).
- Capability tokens in URLs end up in Caddy access logs (S9).

**Privacy and data lifecycle:**
- Project deletion does **not** delete stored PDFs, although the API doc says "everything deleted" (S6).
- `domain_events` (which contain titles), jobs, AI runs and assistant transcripts are never pruned (S15).
- Full text goes to a third-party LLM with no in-app disclosure.

**AI-specific:**
- Assistant tool outputs (abstracts up to 4k chars, document blocks) reach the model **unfenced**, contrary to the architecture doc (ai F3).
- Combined with D#4, a prompt-injected abstract can produce a plausible plan whose summary hides its effect.
- The structured-task path *is* fenced.

**Supply chain:** strong, no findings of note (pinned actions, deny, audit, minimumReleaseAge, checksummed PDFium, digest-pinned bases, non-root).
- `publish-images.yml` interpolates `inputs.tag` into `run:` (template injection, limited to dispatchers), and pushes `:latest` on every tag including pre-releases (ops F8).

**Largest security risk:** an exposed instance, or one reached via DNS rebinding, is a fully privileged, unauthenticated API holding an LLM key and outbound-HTTP/email capability.

---

## G. Architecture review

**Current architecture vs. declared.**
- *Declared* (ADR 0002, `architecture.md`): domain → application (use cases and ports) → adapters.
- *Actual:* a transaction script (arch A1):
  - `deepref-application` has **zero `async fn`**: pure rules plus 10 port traits;
  - `deepref-postgres` (26.7k non-test lines, 639 queries) is the real application layer;
  - plpgsql holds 38 functions and 24 triggers, mixing invariants with *policy* (visibility, dispatch, transition tables);
  - handlers run 61 queries;
  - the worker's `store.rs` has 47 more.

**Accidental architecture:**
- **Speculative ports (A2).** `DocumentRepository`, `OcrEngine` and `FullTextResolver` have zero implementations. `JobQueue`, the provider traits, the `DocumentStore` trait and `ReviewScheduler` are bypassed by production callers. Only the AI ports (`AiGateway`: 14 impls) earn their keep.
- **Vestigial crates (A3).** `core` (mostly re-exports and unused types), `events` (a NATS-era subject catalogue with no broker), `graph/upsert.rs` (unused), `telemetry` (115 lines). `crossref`, `providers` and `grobid` are three adapter crates with three Crossref clients between them (A4).
- **Config bypass (A4).** 32 `std::env::var` reads skip `deepref-config`, so SMTP and parser config fail at first use rather than at startup.
- **Competing patterns:** three run engines (A5); two import pipelines (`/ingestions` vs `/imports`, which share code via a *route* module calling a sibling route); two provider wire stacks for the same provider (ai F7); assistant read tools implemented twice (http `chat.rs` and postgres `assistant_host.rs`).
- **God modules (A7):**

  | File | Lines | What it mixes |
  |---|---|---|
  | `postgres/workflows.rs` | 2,390 | CRUD, webhooks, secrets, run engine, scheduler, plus direct reads of screening and report tables |
  | `worker/processor.rs` | 1,923 | everything, including the whole review executor |
  | `worker/workflows/nodes.rs` | 1,814 | every workflow node |
  | `ExtractionScreen.svelte` | 1,787 | |
  | `AutomationBuilder` | 1,742 | |

- **Boundary checks guard the wrong boundaries (A8).** xtask lets http and worker depend on anything, so SQL in handlers passes every check.
- **Stringly typed seams (A6).**
  - 28 `String` status fields in postgres rows, mirrored in 142 DTOs and then hand-listed again in TS.
  - Job kinds are string literals; one, `recompute_prisma`, is a dead no-op.
  - `project_id: Uuid` appears 289 times vs `ProjectId` 154 times.
  - Only 33 compile-checked `query!` macros vs about 1,661 runtime queries.

**Deepening candidates:**
1. **One `JobLease` / fenced-commit primitive** used by every handler. Today ingestion, automations and review runs each fence differently and workflows not at all.
2. **One durable-run primitive** (run + typed steps + jobs as executor), replacing three engines.
3. **One `classify(sqlx::Error)` → `PersistenceError` → `ApiError`** mapping.
4. **Context use-case modules** (`app::screening`, …) that HTTP, worker and assistant all call, instead of each recomposing SQL.

---

## H. Code quality and simplification

Transformations that remove whole *categories* of complexity:

| Transformation | What disappears |
|---|---|
| **Contract the legacy ingestion plane** (data §5.2) | `legacy_citations`, `project_works`, `ingestion_items`, `processed_events`, `dead_letter_records`, `domain_events`/`domain_tombstones`, the in-job NATS emulation (`DeliveryAction::Nak`, `MAX_DELIVERIES`), two run tables that drift, and about half the per-DOI write set |
| **Flatten the AI execution stack** (ai §5 synthesis) | automation coupling for review runs, `review_step_attempts`/`artifacts`/`lineage`/`finalizations`, `workflow.json` (validated and hashed but not driving execution), one of three idempotency layers, the audit/repair loop |
| **Collapse the crate graph** (arch Option C) | about 8 crates, A2 ports, `core`/`domain` confusion, duplicate Crossref clients |
| **Typed `JobKind` + typed state enums at the row boundary** | dead job kinds going unnoticed, the TS re-listing of server enums, stringly comparisons in Rust |
| **Delete `PolicyEngine`** (always called with the default policy) | about 120 lines plus an unreachable `ReversibleMetadataWrite` path, replaced by a `const fn decision(tool)` |
| **Delete the assistant JSON-command path and `/assistant/tools*`** (no web caller) | about 1,200 lines and the duplicate read-tool implementation |

AI-slop and over-engineering signals (systemic, not per-file):
- ports created before a second implementation existed;
- comments and ADR prose describing architecture that is not used;
- source-grep "tests" counted as integration coverage (ops §4);
- three identity schemes for calibration in about 3.5 months with no producer;
- a 2,279-line build-time fingerprint module (`review/build_support/fingerprint.rs`) whose only output is audit-only provenance that could be a git SHA plus a lockfile hash.

Textual duplication is low (jscpd 2.49%). **The duplication is semantic: parallel models, not copy-paste.**

---

## I. Data and API review

**Domain model.** The identity model is sound: records → reports → studies, DOI is not the key, and resolution never deletes. Inconsistencies:

| Phase | Unit of assessment |
|---|---|
| Screening | per report |
| Appraisal | per report (but RoB 2 is per result) |
| Extraction | per study |

`ai_reviewer_decisions` models "second reviewer" as an AI-only concept (`source IN ('ai','workflow')`, `resolution IN ('kept_human','adopted_ai')`). That is the schema blocking dual human screening.

**Source of truth.** Strong for screening and PRISMA. Weak for:
- run status (two tables, recomputed racily);
- graph metrics (uncoalesced recompute; the reconciler never repairs a dead last recompute);
- report metadata (`ensure_v2_report` overwrites `reports.title` and `abstract` from `works` on every re-fetch, which would clobber curated edits; open question).

**Migrations.**
- Append-only, explicit, and checked against `.sqlx` in CI.
- But migration *data* tests never run (D#1).
- Readiness checks `≥ 10` (D#11).
- The numbering gaps (0032–0039 etc.) came from a squash.
- No policy is documented for N/N+1 coexistence (expand/contract). 0058 removed a `suspended` state, so an N-1 worker against an N schema is undetected.
- 0056 fail-closes AI automation with **no supported recalibration path** (ops F16).

**API shape:**
- 166 operations with cursor pagination on collections.
- A consistent error body `{code, message, correlation_id, details}`, but its semantics are inconsistent (D#12):
  - 400 for transient DB errors;
  - a random correlation id;
  - `RowNotFound` → 404 even when it means an internal invariant failure.
- 202 for cancelling a nonexistent ingestion.
- Actor identity is carried in a header on most routes but in the body on deduplication.

**Retry and idempotency.** Three different idempotency layers for AI with different failure semantics (ai §1.3):
- The job-level `dedupe_key` conflates "in-flight" with "ever" (data F8).
- At the client, screening is retry-safe thanks to the revision CAS.
- Plan confirmation is not.
- Workflow side effects (HTTP, email) can run twice under lease loss (data F3).

---

## J. Frontend and UX review

**Already good:**
- URL-addressable state (deep links to a report or filter);
- careful keyboard shortcuts with suppression in inputs and overlays;
- 409 reconciliation that shows the authoritative state;
- lazily loaded pdf.js and sigma;
- a skip link and focus work;
- lint 0 errors / 63 warnings, svelte-check 0 errors, vitest 535 + 26 green.

**State and data:**
- D#5 and D#13.
- The assistant bypasses svelte-query and the generated client and hardcodes `x-actor-id: local-user` (contradicting the reviewer id used elsewhere) (fe F9).
- Infinite queries refetch every loaded page on each poll (full text every 15 s, documents every 5 s after parsing) (fe F8).
- Sidebar navigation carries query params across views (fe F11).
- Crossing the 768 or 1024 px breakpoint remounts trees and loses drafts (fe F12).

**Bundle.** `@xyflow` (about 193 KB raw) ships in the root layout chunk on every route via the `@deepref/ui/layout` barrel (fe F7).

**Design system.** Lint passes, but drift lives where the linter can't see it (fe F14):
- `<style>` blocks;
- a bespoke `DecisionChoice` button;
- copy-pasted filter chips;
- 8 native selects;
- a hand-rolled progress bar.

**Accessibility (fe F16).** Gaps on custom surfaces: the graph canvas, the workflow canvas, live regions for async jobs, and status conveyed by colour on some badges.

**Information architecture (prod P7, §4.6):**
- 16 sidebar items across 5 groups, with Graph, Recommendations, Automations and Assistant at the same level as screening.
- Jargon leaks into the UI: "Ingestions", "Proposal", "Review run", and AI-first "cohort / Draw fixed audit / Evaluate once".
- Steps exist for the architecture rather than the user: manual deduplication runs, the "Refresh project statistics" template, typing a reviewer id, and recalibration after deploys.

---

## K. Performance review

**Measured problems (dev DB data, data F7):**
- 225 `recompute_metrics` jobs for 227 fetches;
- 78 (35%) carry serialization failures;
- each recompute loads the whole project graph and issues one UPDATE per report.

**Structural risks:**
- O(N²) per-run status recompute (about 14 scans of `ingestion_items` per completed item);
- unbounded depth;
- a claim loop capped at about 10 jobs/s per worker, with a recovery UPDATE every tick (data F15);
- the project-row mutex across screening and enqueues;
- the HNSW index unusable by the hybrid query, though nothing currently embeds (ai F12);
- the PDF viewer renders every page canvas at device pixel ratio (fe F18);
- xyflow on every route.

**Hypotheses (need measurement):**
- screening latency under concurrent imports on one project (pg_locks waits);
- LCP and INP on the screening route with a 10k queue;
- worker throughput with batch claim plus `LISTEN/NOTIFY`.

**Recommended instrumentation:**
- RED metrics per route template;
- job queue age and depth by kind;
- LLM latency and tokens by task and outcome;
- a CI bundle-size budget for the root chunk;
- `pg_stat_statements` in dev.

---

## L. Observability and operations

| Question | Answerable today? |
|---|---|
| What is broken? | Partly: `/health/dependencies` (PG ping, job backlog, dead and stale counts) |
| Who is affected? | No. No per-project error metrics, and the correlation id is random |
| When did it start? | No. No metrics, no access log at INFO |
| Which requests or jobs? | Jobs only via manual SQL on `jobs.last_error`. Failures are not logged |
| Which dependency? | AI: no. Gateway errors collapse to "provider request failed" (ai F10). Others: logs only |
| Getting worse? | No |
| Did the deploy cause it? | No. `DEEPREF_BUILD_SHA` is set in the image and read nowhere |
| What next? | Runbooks are code walkthroughs with about 27 `file:line` references each, not symptom → query → action |

**Missing:**
- a span per request carrying the correlation id;
- a span per job with `job_id`, `kind`, `project_id` and `attempt`;
- `warn!` on job failure and `error!` on dead-lettering;
- a text-map propagator;
- RED metrics and queue gauges;
- `service.version`;
- a worker heartbeat;
- AI `error_class` and a per-task/model cost view.

**Noisy:** always-JSON logs locally.

---

## M. Testing and verification

**Current confidence:**
- High for pure Rust logic (about 850 unit tests in about 4.8 s; ai and review alone 184).
- Medium-high for the web unit and component layer.
- **Near zero, in CI, for everything touching PostgreSQL** (D#1).

**False confidence:**
1. DB suites passing in about 10 ms without a database (D#1).
2. Source-grep "tests" (`crash_boundaries.rs`, `persistence_boundaries.rs`, `global_throttle.rs`, `migration.rs`) that assert substrings.
3. A required "E2E" check that is a fully mocked browser plus vacuous DB tests.
4. An "AI gold-set gate" that scores a static predictions file and never calls a model.
5. Mutation testing: 43% kill rate. Survivors include `validate_judgment → Ok(())` and `validate_answer_schema → Ok(())`.

**Blind spots:**
- re-requesting a failed review run;
- confirming a stale plan;
- orphaned assistant runs;
- metering a timed-out call;
- the AI-first sweep under an identity error;
- concurrent final-item completion;
- workflow-step double execution;
- poison jobs;
- schema-version readiness;
- no real-backend smoke journey (ops F5);
- UI package tests never run in CI, and `packages/**` changes skip all web checks (ops F4).

**Broken lanes.** The scheduled fuzz, benchmarks and ts-mutants jobs have failed on 4 of 4 runs since they were added (each fix is one line, ops F6). `rust-mutants` succeeds only via `|| true`.

---

## N. Developer experience and repository health

**Onboarding:** README → `mise install` → `just bootstrap` → `just dev` is clean. Hidden prerequisites:
- `sentrux`, `cargo-nextest`, `sqlx-cli`, `cargo-llvm-cov` and `cargo-deny` are not pinned in `.mise.toml`;
- libpdfium has no local recipe, so PDFs fail lazily as job errors.

**Unsafe sharing:** `just test-integration` runs against the **dev database**, and tests claim real dev jobs (ops F13).

**Repository hygiene:**
- About 64 GB of untracked worktrees and `target/` directories live *inside* the repo. `/home` hit 100% during this review, and I had to remove a merged worktree's 38 GB `target/` with your permission.
- `.dockerignore` doesn't exclude `.worktrees`/`.claude`, so local Docker builds send nested manifests.
- `observability/` contains only node_modules, `examples/` is empty, and the leftover `services/api/` directory is root-owned.

**Agent guidance:**
- `AGENTS.md` links to a nonexistent `/home/luzes/.t3/worktrees/...` path and describes a nonexistent `packages/core`.
- Its verification section lists only the pnpm gates, for a mostly-Rust repo.
- `skills-lock.json` locks 6 of 10 skills.

**Documentation drift:**
- `architecture.md` opens with the old product ("maps article citation networks").
- `api.md` references a deleted Helm job.
- `rete-workflow-migration.md` describes a removed editor as in progress.
- Handoff docs are agent work-plans sitting among reference docs.

**Tooling proportion:** sentrux, fallow, jscpd, cargo-shear, deny, mutants, fuzz, criterion, bacon, taplo, actionlint and shellcheck are all present, but several gates are local-only, report-only or broken. **The quality tooling is broader than the quality signal it produces**; one working DB lane is worth more than all the scheduled lanes combined.

**Environment:** this shell ran Node 22 against `engines >= 24` because I invoked commands outside `mise exec`. That's my environment, not a repo defect, but nothing warns about it.

---

## O. Product and feature opportunities

### High-confidence improvements

| # | Improvement | Value | Cost | Architecture fit |
|---|---|---|---|---|
| H1 | **Trusted reviewer identity plus a project roster.** Minimal version: accept `X-Forwarded-User` from oauth2-proxy or Authelia and refuse free-text actors when it is configured. | Precondition for teams, credible AI-first, honest methods claims | Medium (small version) | `routes/actor.rs` is the single seam. Every event already has `actor_kind`/`actor_id`. |
| H2 | **Dual human screening**, by generalising the AI second reviewer into `reviewer_opinions`, with `screening_state` as the consensus projection. Reuses the Conflicts panel, kappa and the reveal logic. | Table stakes for Cochrane-grade reviews | Medium–High | Natural (event log + projection). Exposes the AI-specific schema in 0029. |
| H3 | **Search log plus source-aware PRISMA** (database, platform, date, query on import; per-source identification; an "other methods" column for citation chasing) | Every published review needs PRISMA-S | Low–Medium | Data already in `acquisition_runs`/`record_provenance`. Exposes the two import pipelines. |
| H4 | **Prioritised title/abstract screening** (TF-IDF + logistic regression in a worker job, score snapshots recorded for audit) | High for more than about 1k records | Medium | Queue sort and cursor are designed for it. Record ordering as an exposure. |
| H5 | **Bulk open-access PDF retrieval** button | Removes the biggest manual chore | Low | The `attach_pdf` node already exists |
| H6 | **Appraisal coverage**: ROBINS-I v2, QUADAS-2, a robvis CSV, and suggesting only tools that exist | High | Low per tool | JSON definitions |
| H7 | **Meta-analysis-ready extraction** (`choice` type, wide CSV; arm/outcome groups later) | Medium–High | Low (wide CSV) | The unique-per-study-field constraint blocks repeating groups |

### Strategic opportunities

- **S1: Methods & reproducibility package.** This is the signature artifact: generated methods text, an AI-use disclosure table (model, prompt versions, autonomy per task, blinding, kappa, verified-AI counts), protocol diffs, the search log, PRISMA, and audit and manifests in one zip.
  - ADR 0005 §1 already writes the permissible claim for each mode, and `audit_export.rs` already joins the data.
  - **No competitor has this.** Cheap relative to its value.
- **S2: "How this stage is screened" as a first-class choice.** Single, dual human, human plus blinded AI, or AI-first gated by scale. Each shows its methods sentence, forecast workload and cost.
- **S3: Living review mode.** Saved searches (H3 data) tied to the protocol, "re-run now", a "new since last update" filter, and PRISMA update cycles.
- **S4: Decide the deployment model explicitly** (self-hosted single user vs hosted multi-user) before investing further in automations. It changes the threat model of the workflow engine entirely.

### Small high-leverage changes

- Keyword highlighting in the title/abstract reader.
- `event.repeat` guard and a synchronous in-flight lock for decisions (fixes D#5).
- Show human–AI kappa on the PRISMA page.
- Run fuzzy dedup automatically after import.
- Default citation depth to 0 for systematic-review projects, with a volume estimate.
- Rename leaked jargon.
- "Retry" and "Regenerate" on AI runs (with D#7).
- A plan card that lists the actual records, with per-item deselect (with D#4).
- Fix the studies list member counts.

### Product simplifications

| ID | Change |
|---|---|
| P1 | Expose 4–5 purpose-built automations. Put the canvas behind "Advanced". **Park Slack, HTTP, webhook and inbound-email nodes** until users ask. This deletes the D#8 surface. |
| P2 | Remove or park calibration bundles: no production producer or consumer, yet 4 migrations and a runbook. |
| P3 | Repurpose Recommendations into citation-chasing candidates: works cited by ≥ k included studies and not yet in the project. |
| P4 | Move Graph under Collect, as citation searching. |
| P5 | Narrow the assistant to "ask your corpus, with citations" (read tools). Writes go to the normal proposal queues. |
| P6 | Unify the two import pipelines. |
| P7 | Collapse the navigation from 16 items to about 8. |
| P9 | Align the extraction default (`Act`) with the README promise. Either default to `suggest`, or rename it "AI fills, you verify" and block export while values are unverified. |

### Explorations

- **E1:** Gate AI-first behind scale (ADR §10: ≥ 20% *and* ≥ 500 judgments saved).
- **E2:** Reuse the hypergeometric audit machinery as a **stopping rule** for prioritised screening. That is more widely useful than quarantine-based AI-first.
- **E3:** Draft a PROSPERO/OSF registration from the structured protocol.
- **E4:** Cross-project templates.
- **E5:** Evidence by text selection in the PDF.
- **E6:** Human–human calibration exercises.
- **Batch screening calls** (N records per call) to cut cost 5–10× on large title/abstract stages (ai §6).

---

## P. Zero-based architecture

*Built today, for the same constraints:* single-tenant self-host first, team-capable, one Postgres, correctness and audit over throughput.

```text
crates/
  domain/        pure: typed ids, state enums (sqlx::Type), state machines, policy tables
                 (screening, AI-first CohortState × Trigger → Effect, job lifecycle, autonomy decisions),
                 PRISMA, RoB2/appraisal, dedup scoring, workflow planner
  app/           one module per context — protocol/ imports/ screening/ documents/ review_ai/ automations/
                 use-case fns over &mut PgTx, owning their SQL and read models; contexts call each other's
                 public fns, never tables; plpgsql only for immutability invariants
  runtime/       ONE durable-run primitive: runs + typed steps + jobs (claim_token fencing, max_attempts on
                 recovery, coalescing partial-unique dedupe, attempt history, batch claim + LISTEN/NOTIFY)
  ai/            one wire stack, ai_calls ledger (every attempt incl. timeouts), definitions as Rust fns,
                 identity = hash(derived schema + rendered prompt + validators version)
  integrations/  crossref, pubmed, unpaywall, openalex/europepmc, grobid, smtp — built once from config
  http/          axum: authn middleware (forward-auth or session) → Actor; parse → app fn → serialize; no sqlx
  worker/        exhaustive JobKind dispatch → app fns; PDFium in a sandboxed subprocess; no sqlx
  config/        every env read; production by default
apps/server      composition root, telemetry (RED metrics, spans with correlation_id/job_id)
```

**Data:**
- v2 only, with a single `acquisition_runs` that keeps incremental counters.
- `reviewer_opinions`, with `screening_state` as the consensus projection.
- A `project_members` roster.
- No `domain_events` outbox, since the jobs table is the outbox.
- Retention policies on jobs, AI calls and transcripts.

**Deltas worth migrating toward (in order of payoff/cost):**
1. Identity and authentication middleware (one seam).
2. Hardening the job queue.
3. Contracting the legacy plane.
4. Flattening AI execution onto `ai_calls`.
5. Context modules, piloted on screening.
6. Unifying the run engines, *after* the product decision on workflow scope.

**Not worth migrating:** completing the hexagon (arch Option A). It would double the code and fight the database.

---

## Q. Design-it-twice candidates

### Q1. Where use cases live (arch §5)

| Option | Shape | Wins | Costs |
|---|---|---|---|
| A. Real hexagon | async services in `application` over ports, Postgres implements all ports | matches docs, fakes-based tests | about 2× code, unit-of-work plumbing, fakes drift from PG locking semantics |
| **B. Vertical contexts over PG (recommended)** | `app/<context>` owns SQL + use cases; pure `domain`; http and worker have no sqlx | mostly a *move*, one context at a time, rules get one home | needs real DB tests (D#1 first) |
| C. Collapse crates only | merge 8 small crates, delete ports | days of work, immediate win | leaves rules in six places |

**Synthesis:** C first, then B piloted on screening.

### Q2. Durable execution (data §5.1, arch A5)

| Option | Shape | Wins | Costs |
|---|---|---|---|
| **A. Harden the PG queue (recommended)** | claim_token, recovery respects attempts, active-dedupe partial index, attempt history, fenced-commit helper | small, keeps in-tx enqueue | still own the queue code |
| B. Library queue (pgmq/apalis-style) | adopt a library | free visibility timeouts and archiving | may lose transactional enqueue, new dependency |
| C. Per-aggregate state machines + thin scheduler | each aggregate owns status/attempt/lease | explicit fencing | lease logic scattered unless factored |

**Synthesis:** A now. Then collapse the three engines into one run primitive whose steps are typed handlers (arch A5).

### Q3. AI execution stack (ai §5)

| Option | Shape | Wins | Costs |
|---|---|---|---|
| A. Flatten: "AI job = function" | `ai_jobs` → `ai_calls` → proposals, definitions as Rust fns, generation column | deletes about 4 tables + automation coupling, F1 trivial | loses node-level lineage (mitigated by per-call hashes) |
| B. Make the DAG real | interpreter over `workflow.json`, own job kind | honors ADR 0004 literally | most code, interpreter risk for 2 graph shapes |
| C. Effect-log centric | rig-cassette-style logs as the audit artifact for structured and assistant runs | one runtime, natural replay | couples audit to a third-party format, event-sourcing weight |

**Synthesis:** A's execution shape, plus B's manifest identity (made truthful per ai F8), plus C's per-call ledger.

### Q4. Identity (product H1, security S1)

| Option | Shape | Wins | Costs |
|---|---|---|---|
| **A. Trusted forward-auth header (recommended first)** | `X-Forwarded-User` from oauth2-proxy or Authelia, with a Host allow-list and a loopback bind by default | days of work, real identities for self-hosted teams | requires a proxy, no in-app account management |
| B. Built-in sessions | passkeys or OIDC, `users` + `project_members` | hosted-ready | large: password and recovery flows, CSRF |
| C. Per-project invite tokens | bearer token per reviewer | no proxy needed | token handling and rotation, weaker |

**Synthesis:** A now, B only if S4 picks hosted.

---

## R. Deletion plan

| Item | Confidence | Prerequisite |
|---|---|---|
| `recompute_prisma` job kind, `prisma_snapshots`, `crossref_raw_records`, `jobs_dedupe_kind_idx`, `jobs.state='failed'` value | High | confirm no queued rows or producers |
| `application` ports with 0 impls (`DocumentRepository`, `OcrEngine`, `FullTextResolver`); bypassed ports (`JobQueue`, provider traits, `DocumentStore` trait, `ReviewScheduler` + `MemoryReviewScheduler`) | High | rewrite 2 job tests against the free functions |
| `deepref-core` dead types, `graph/upsert.rs`, unused `events` subjects; fold `core`/`graph`/`telemetry`, merge `crossref`/`providers`/`grobid` | High | none |
| `RigGateway`, `RigEmbeddingGateway`; embeddings table + HNSW + gateway (or finish them) | High (dead today) | product decision on semantic retrieval |
| Assistant JSON-command path, `/assistant/tools*`, duplicate read tools, `PolicyEngine` | High | none (no web caller) |
| Inline task `system_prompt`s; stub `schema.json` files (replace with the derived-schema hash) | High | bump identity scheme |
| `candidate_audit` + `semantic_repair` nodes | Medium | offline evaluation on a labeled set |
| Calibration bundles (park: freeze code, drop the policy prose) | Medium | product decision P2 |
| Legacy ingestion plane (`legacy_citations`, `project_works`, `ingestion_items`, `processed_events`, `dead_letter_records`, `domain_events`/`tombstones`, `ingestions` table → view) | High (direction), Medium (timing) | 3-release expand/contract (data §5.2) |
| `build_support/fingerprint.rs` (2.3k lines) | Medium | replace with git SHA + lockfile hash |
| Z.AI dialect and config | Medium | announce a deprecation date |
| Web: `features/workflows/domain` (2.9k lines, test-only), demo routes, `vitest-examples`, duplicate typewriter, unused `sass`, unreachable `deduplication/+page.svelte`, 15 unused `@deepref/ui` exports, about 15 uncalled generated operations | High | none |
| CI: `ai-eval-security-gates`, duplicate `--tests` run, PR-time jscpd, unused ripgrep install; fix-or-delete fuzz, benchmarks, ts-mutants | High | none |
| Repo: `observability/`, `examples/`, `services/api/`, stale `.jscpd`/`.dockerignore` entries for deleted dirs, handoff docs → `docs/archive/` | High | none |
| Workflow Slack, HTTP, webhook and inbound-email nodes (park) | Medium | product decision P1/S4 |

---

## S. Migration roadmap

**Stage 0: Immediate safety (days)**
1. CI DB lane: `DATABASE_URL` on the job, a `DEEPREF_REQUIRE_DATABASE=1` panic guard, per-test databases (`sqlx::test` or a template DB) or nextest serial groups. Expect, and fix, the interference failures.
2. Screening keyboard guard (`event.repeat` plus a synchronous in-flight flag) (D#5).
3. AI-first: three-way `Current | Stale | Unknown`. Never demote on `Unknown`. Exclude `finalized` from the periodic sweep unless the protocol changed (D#3).
4. Assistant plans: capture revisions at plan time, show the actual records with deselect, gate on autonomy, add an `origin=assistant_plan` provenance column (D#4).
5. Deploy defaults:
   - bind `127.0.0.1` by default;
   - refuse to start on a non-loopback bind without an auth mode configured;
   - add a Host allow-list;
   - make `APP_ENV` default to production in the binary (set `local` explicitly in dev);
   - skip dotenv when not local.
6. Recovery respects `max_attempts`, and the worker survives job panics (D#6, partial).

**Stage 1: Foundations (weeks)**
- Job lease tokens and a fenced-commit helper, including fencing for workflow steps.
- `PersistenceError` classification and the HTTP mapping; a real correlation id; job spans and failure logs; readiness derived from `MIGRATOR`; a worker heartbeat; RED metrics.
- Fix `packages/**` CI filters, the UI tests, and the scheduled lanes.
- One real-backend Playwright smoke lane.
- Retry and Regenerate for review runs (generation column).
- Ledger every LLM attempt; do not retry send-timeouts in the gateway; classify `AiError` in the worker.

**Stage 2: Simplify (weeks)**
- The deletion plan's High-confidence rows (Option C crate collapse, dead ports, dead AI paths, web dead code, CI cleanup).
- `JobKind` enum; all config through `deepref-config`; typed state enums at the row boundary.
- Coalesce metric recompute and write it with one batched UPDATE; bound citation depth.

**Stage 3: Architecture corrections (1–3 months, incremental)**
- Pilot `app::screening` (Option B), with HTTP and assistant calling one function. Tighten xtask rules for that context.
- Contract the legacy ingestion plane across three releases; `acquisition_runs` counters.
- Flatten AI execution onto `ai_calls` (Q3 synthesis), definition by definition. Make the identity hash truthful.
- Revision CAS on extraction, appraisal and studies.

**Stage 4: Product/UX (parallel with Stage 3 once Stage 0–1 land)**
- H1 identity (forward-auth) → H2 dual screening.
- H3 search log and per-source PRISMA; H5 bulk PDFs; H6 appraisal tools.
- Navigation collapse and jargon renames; S1 methods package (small version first).

**Stage 5: Strategic**
- H4 prioritisation plus E2 stopping rules; S2 stage modes; S3 living reviews.
- Unify the run engines after the S4 / P1 decisions; sandbox PDFium; batch screening calls.

---

## T. Recommended change packaging

One PR per line, each independently verifiable:

1. `ci: run DB integration tests with isolated databases` (test helpers + workflow). *First, alone.*
2. `fix(web): guard screening shortcuts against repeat and re-entrancy`
3. `fix(ai-first): never demote on unknown identity; keep finalized cohorts`, with a DB regression test.
4. `fix(assistant): plan-time revisions, record list, autonomy gate, provenance column` (migration + API + UI, in that order).
5. `fix(server): safe deploy defaults` (bind, APP_ENV, Host allow-list, auth-required guard).
6. `fix(jobs): attempt-aware recovery, panic isolation, claim tokens` (schema → queue functions → worker).
7. `feat(api): persistence error classification and real correlation ids`
8. `chore: delete dead ports, crates and AI paths`. Mechanical, large but low-risk. Keep it separate from feature work.
9. Then the context-module pilot, the legacy contraction migrations (one PR per release phase), and AI flattening (one definition per PR).

---

## U. Verification plan

| Change | Proof |
|---|---|
| CI DB lane | CI DB suites take seconds, not ms. A deliberate inversion of `leased_until > now()` in `complete_job` fails CI. |
| Keyboard guard | Component test: dispatch 5 repeated keydowns, assert 1 mutation. Playwright: hold "e" for 1 s, assert one decision. |
| AI-first | DB test: make `preview_screening_identity` fail (drop the route), run the sweep, and assert the finalized cohort and its exclusions are unchanged. |
| Assistant plans | HTTP test: change a record after plan creation, confirm, assert that item is rejected as stale. PRISMA excludes `origin=assistant_plan` from the human stratum. |
| Deploy defaults | Config test: no `APP_ENV` and no `DATABASE_URL` → error. Starting on `0.0.0.0` without auth → refuses. A request with a foreign `Host` → 421/403. |
| Queue | `max_attempts=1` + expire + recover → `dead`. A stale claim token cannot complete. Two concurrent workflow `handle_step` calls → one execution. |
| Error semantics | `classify` unit tests per SQLSTATE. The error body id equals the request `x-correlation-id`. |
| Metrics recompute | Same seed ingestion: number of recompute jobs and serialization-failure rate before and after; graph API golden output unchanged. |
| Legacy contraction | `n_tup_ins` per DOI before and after; a drift query between run tables returns 0; graph API goldens unchanged. |
| Bundle | Root chunk size budget in CI; xyflow absent from the root chunk. |
| Observability | Force a job failure and assert a log line with `job_id`; readiness test against the max embedded migration. |

---

## V. Unknowns and limitations

- **Not run:** the full stack and the browser app (static review plus committed visual baselines only), so there are no live LCP/INP measurements. Security exploit paths marked "(inferred)" in the security report (DNS rebinding, mail relay, cross-site multipart) are mechanism-verified, not exploited. Load and concurrency races (data F5, F10) were not reproduced. No production telemetry or deployment exists to inspect.
- **Partially run:**
  - The DB suite ran once in parallel (837/856) and the failing binaries once serially (33/33). The full suite was not run serially end to end, so other latent interference-masked failures are possible.
  - The web bundle analysis used the existing `apps/web/build` from 2026-10-08, because the rebuild failed while `/tmp` was full.
- **Environmental interference:** `/home` reached 100% during the review (untracked worktree `target/` dirs). With your permission I removed `.worktrees/rig-044-cassette/target` (38 GB, regenerable). `/home` is at 99% again (about 3.4 GB free) after the test builds.
- **Assumptions:** that single-user self-hosting is the current deployment model (no deploy manifests exist since the Helm chart was deleted), and that dev-DB data patterns (serialization failures, run drift) are representative.
- **Needs user or product research:** solo vs team users; typical title/abstract volumes (they decide AI-first's value); journal AI-disclosure requirements; actual automation node usage; whether Graph and Recommendations are used; analysis export targets; hosted vs self-hosted.
- **Owner questions:**
  - Is reopening finalized AI-first exclusions on a non-protocol identity change intended? The ADR says no; the code does it.
  - Is the hexagon a target or scaffolding?
  - Is `events`/`domain_events` meant for a future bus?
  - Which plpgsql functions are invariants vs policy?
  - Is node-level artifact lineage a hard audit requirement?

---

## Final prioritization

| Priority | Recommendation | Category | Impact | Effort | Confidence | Why now |
|---|---|---|---|---|---|---|
| 1 | Run the DB suite in CI with isolated databases and fail when the DB is absent | Defect (verification) | Very high | Low | Very high | Every other change depends on this net; it's currently a false green |
| 2 | Stop the three integrity leaks: keyboard re-entrancy, assistant plan bypass, AI-first destructive demotion | Defect | Very high | Low–Medium | High–Very high | Each can silently alter scientific decisions, the product's core asset |
| 3 | Safe deploy defaults + trusted identity seam (forward-auth, loopback bind, Host allow-list, auth-required guard) | Defect / Architecture | High | Medium | Very high | Images are published, and every integrity feature assumes identity |
| 4 | Harden the job queue (attempt-aware recovery, panic isolation, claim-token fencing, workflow fencing) | Defect | High | Medium | High | Poison inputs (PDFs) can stall all background work |
| 5 | Observability minimum: correlation, job spans and logs, RED metrics, truthful readiness | Behavior-preserving | High | Low–Medium | High | Today failures are invisible except by SQL |
| 6 | Dual human screening on the generalised second-reviewer model | Product | Very high | Medium–High | High | Table stakes for the target users; the AI-first audit needs it too |
| 7 | Methods & reproducibility package (small version first) | Product | High | Medium | Medium–High | Turns the deepest existing investment into visible user value |
| 8 | Simplify: crate collapse, dead ports and AI paths, legacy-plane contraction, park the workflow nodes | Architecture | High | Medium | High | Shrinks the surface before deeper refactors and removes the secret-exfiltration surface |
| 9 | Flatten AI execution onto an `ai_calls` ledger with retry/regenerate and truthful identity | Architecture | Medium–High | High | Medium–High | Fixes stuck runs, cost blind spots and three idempotency layers at once |

---

## Answers to the 44 questions

1. **Strongest architectural decision:** PostgreSQL as the single source of truth *including* the job queue, so state, follow-up work and audit commit atomically in one transaction. Close second: PRISMA as a live projection.
2. **Weakest:** building scientific-integrity mechanisms (blinding, fresh auditors, dual-reviewer statistics, human-confirmation as the AI safety argument) on a self-asserted, per-tab identity string.
3. **Code that could disappear entirely:** the legacy ingestion plane and its NATS emulation; speculative ports; `core`/`events`/`graph`/`telemetry` as crates; Rig gateway/embedding dead paths; the assistant JSON-command path and `PolicyEngine`; the audit/repair loop; `features/workflows/domain`; the broken CI lanes.
4. **Least justified abstraction:** the `application`-layer port traits (zero or bypassed implementations). In the AI stack: `workflow.json`, hashed and validated but not driving execution.
5. **Missing abstraction:** a *job lease* (claim token + fenced commit) shared by every handler; also a persistence-error classifier, and a trusted `Actor` produced by authentication rather than parsed from headers.
6. **Complexity in the wrong place:** policy in plpgsql (visibility, dispatch, transitions); use cases in the "postgres" crate; SQL in handlers and in `worker/store.rs`; the review executor inside `processor.rs`; workflow nodes reading other contexts' tables.
7. **Module that should become deeper:** the job/run runtime (one primitive with fencing, retry, dead-lettering and history), and per-context use-case modules, starting with screening.
8. **Module that should probably disappear:** `deepref-core` (and `deepref-events` with `domain_events`). Product-wise, the general-purpose workflow node set beyond 4–5 templates.
9. **Where the type model permits impossible states:** stringly status columns and row fields (jobs, cohorts, documents, workflow nodes); `jobs.state='failed'`, which is allowed but never written; `actor_kind` that any client can set to `system`; AI-first cohort demotion, where one function serves all triggers regardless of status; `Uuid` vs typed IDs.
10. **Business logic duplicated:** assistant read tools (http `chat.rs` vs `assistant_host.rs`); three Crossref clients; two SSRF block-lists; two query sanitizers; 409/optimistic logic implemented twice in the web for the two screening stages; server enums re-listed in TS; three idempotency schemes and three run state machines.
11. **Unclear system boundaries:** application vs postgres; the two import pipelines (route calling route); workflow engine vs every other context; assistant vs governed AI path (plans write directly).
12. **Where partial failure corrupts or confuses state:** the AI-first sweep on transient errors (destructive); terminal review runs (stuck forever); post-commit autonomy apply; second-review claim/schedule split; plan confirmation (partial); ingestion final-item race; cancel updating one of two run tables; orphaned assistant and AI runs.
13. **Largest security risk:** an unauthenticated, fully privileged API that binds `0.0.0.0` by default, has no Host check (DNS rebinding), and holds an LLM key plus outbound HTTP/email and stored plaintext secrets.
14. **Largest reliability risk:** poison jobs (especially PDFs parsed in-process) that are reclaimed forever and can crash every worker, combined with no logging of job failures.
15. **Largest performance risk:** ingestion fan-out with unbounded depth × O(N²) per-run status recompute × an uncoalesced full-graph metric recompute per item.
16. **Currently unobservable:** which request or job failed and why (no access log, random correlation id, unlogged job failures), AI spend per task/model and failure class, worker liveness, schema/binary mismatch, and the effect of a deploy.
17. **Tests that give false confidence:** the 30k lines of DB tests skipping in CI; source-grep "tests"; the mocked "E2E" required check; the static "AI gold-set gate"; validators that mutation testing shows can be replaced with `Ok(())`.
18. **Painful developer workflow:** integration tests against the shared dev DB with cross-test interference; unpinned tools (sentrux, nextest, sqlx-cli); no local PDFium; about 64 GB of in-repo worktrees filling the disk; three test layers with misleading names.
19. **Painful user workflow:** running a team review (impossible); documenting searches for PRISMA (manual); collecting full texts (one at a time unless you build an automation); setting up AI (env vars + global settings + a screening tab); AI-first (a statistical wizard in raw terminology).
20. **Feature creating more complexity than value:** the general-purpose workflow engine (about 20k LOC, secrets, webhooks, inbound email, Slack, HTTP), followed by calibration bundles (no producer or consumer).
21. **Obvious missing feature:** dual independent human screening with conflict resolution, which requires real reviewer identity.
22. **Non-obvious high-leverage feature:** the Methods & Reproducibility package, generated from data DeepRef already records. Runner-up: the hypergeometric audit reused as a stopping rule for prioritised screening.
23. **Feature to make dramatically simpler:** automations, reduced to 4–5 purpose-built toggles. AI-first framed as a guided wizard. The assistant narrowed to cited Q&A.
24. **Should never have been configurable:** `rate_limit_per_second`, `max_concurrency` and citation depth without bounds; `API_CORS_ALLOW_ANY`; per-project budgets that any client can raise to $100k; the retired Z.AI provider kept "for emergencies".
25. **Should become configuration (validated at startup):** SMTP, PDF parser and abstract-enrichment settings that are read ad hoc from the environment; auth mode / trusted proxy header; allowed Hosts; retention windows; an AI workspace-wide spending cap.
26. **Move from client to server:** article search, sort and counts (currently over loaded pages); import-history scoping; plan item rendering (derive from args, not the model summary); reviewer identity (from authentication, not sessionStorage).
27. **Move from server to client:** little. Possibly keyword highlighting and wide-CSV pivoting (both cheap to do client-side). The server-centric design is right.
28. **Should be synchronous:** applying the autonomy opinion in the same transaction as proposal finalization; scheduling second reviews in the same transaction as their claim; fuzzy dedup immediately after import (as a coalesced job, but not a manual user step).
29. **Should be asynchronous:** import parsing (currently synchronous in async handlers); PDF parsing in a sandboxed subprocess; assistant plan execution for large plans (as a job with progress).
30. **State to stop storing and derive:** `ingestions` as a separate row (a view over `acquisition_runs`); per-run status counts (incremental counters or derived); the `domain_events` revision used for metric freshness (a project sequence); `prisma_snapshots` (already unread).
31. **Data needing a stronger source of truth:** acquisition/ingestion run status; report metadata (`works` vs `reports`); actor identity; graph metrics freshness.
32. **First dependency to remove:** `rig-cassette` from the worker's direct dependencies (keep it as a test fixture inside `deepref-ai`). At the infrastructure level, pgvector as a hard requirement, since nothing embeds today.
33. **Custom code to replace with an existing capability:** the in-job NATS delivery emulation (the jobs table already provides it); the build-time fingerprint module (git SHA + lockfile hash); hand-rolled SQLSTATE checks (one classifier); identity (a forward-auth proxy).
34. **Underused existing capability:** the audit and manifest data (no user-facing artifact); human–AI agreement statistics (hidden in the Conflicts tab); the screening queue's sort machinery (no prioritisation); the existing per-step fencing pattern in automations (not reused by the job layer); `sqlx::query!` compile-time checking (33 of about 1,700 queries).
35. **What fails catastrophically at 10× usage:** ingestion with depth ≥ 2 (quadratic status work + per-item full recompute); per-project screening throughput under the project-row mutex during imports; an unpruned `jobs` table slowing recovery scans; sweeps over all projects on every worker every 30 s.
36. **What becomes painful with 10× more developers:** god modules (`workflows.rs`, `processor.rs`, `nodes.rs`) as merge-conflict hotspots; rules in six places; 254 routes registered in one function; a CI whose DB lane is green regardless; hand-mirrored enums across Rust, SQL and TS.
37. **What I'd design differently from the start:** authentication-derived actors from day one; one durable-run primitive with claim-token fencing; v2 data model only; context modules over PostgreSQL rather than an aspirational hexagon; AI definitions as Rust functions with a per-call ledger; a DB test harness with per-test databases from the first migration.
38. **Should NOT be redesigned despite imperfections:** the screening aggregate; PRISMA as a live projection; the single-binary, PostgreSQL-only topology; the AI output validation pipeline and proposal CAS; the record/report/study identity model; the supply-chain/CI pinning setup.
39. **Highest-leverage behavior-preserving refactor:** the job-lease primitive (claim token + fenced commit + attempt-aware recovery) used by every handler. Close second: the Option C crate collapse plus dead-port deletion.
40. **Highest-leverage intentional product change:** trusted identity → dual human screening, making "second reviewer" a role rather than an AI concept.
41. **If only three initiatives:**
    1. a verification and safety foundation: the CI DB lane, the three integrity fixes, queue hardening and observability minimum;
    2. identity and team screening (H1 + H2);
    3. simplification: crate/port deletion, legacy contraction and the workflow-scope decision.
42. **If only one:** identity and team screening, *provided* the Stage 0 CI DB lane and integrity fixes ship first (they are days, not an initiative). It is the only item that changes who can use the product and whether its AI claims are defensible.
43. **A question the architecture made unnecessarily hard to answer:** "Do our persistence and concurrency guarantees actually hold?" The tests existed but were silently skipped, rules are spread across six layers, and no metrics or logs record job failures. Runner-up: "who made this decision?", which is unanswerable with confidence.
44. **What we are not thinking about** (after a final adversarial pass):
    - **Methodological validity of the AI pipeline as shipped.** The audit/repair loop optimises rationales until an anchored auditor agrees, and there is no real model evaluation harness. The scientific claims depend on behaviour nobody measures.
    - **The ordering effect.** Queue order (and future prioritisation) is a form of exposure that biases screening, and it is not recorded the way AI exposure is.
    - **Data residency and consent.** Full texts go to a third-party LLM provider under a consumer subscription. Institutional data policies and publisher licence terms for text-mining PDFs are not surfaced.
    - **Backup and restore.** There is no project export/import, no documented backup, and PDFs are orphaned on delete. A self-hosted user's multi-month review is one disk failure from loss.
    - **Reproducibility over time.** Provider model revisions behind an OpenAI-compatible endpoint can change silently under a fixed model name. Manifests record the name, not the weights. The methods package must say so.
    - **The bus factor of agent-authored complexity.** Much of the surface (ADR 0005's statistics, three run engines, calibration schemes) appears to have been built in fast agent-driven passes. Its owner may be the only person who can judge which parts are load-bearing, and that judgment needs to be written down before deletion work starts.
