# Review 1: Data architecture, persistence, durable jobs, concurrency

Reviewer lens: crates/app (migrations, src, tests), services/worker, crates/application, crates/domain, crates/events, apps/server, and how crates/http-api handlers write state.
Method: static tracing of SQL and Rust, git history, CI config, and **read-only** queries against the local dev DB (`deepref-local-postgres-1`, schema at migration 48, inside `BEGIN READ ONLY`). I asked to create an isolated scratch database to run SQL experiments and the DB integration tests; the permission system denied it. So the concurrency findings below rest on code reading plus PostgreSQL semantics, and each one is labelled for that.

---

## 1. Mental model

### Planes
- **Legacy citation-ingestion plane (DOI-keyed, still the live write path):** `ingestions`, `ingestion_items`, `works` (the Crossref cache), `project_works`, `legacy_citations` (renamed from `citations` in 0007), `unresolved_references`, `fetched_citation_facts`, `fetched_unresolved_reference_facts`, `doi_fetch_state` (per-DOI lease), `processed_events` (per-event lease, left over from NATS), `dead_letter_records`, `provider_rate_state`, `domain_events`/`domain_tombstones` (envelopes from the event bus era), `projection_state`, `metric_snapshots`.
- **v2 evidence workspace (UUID-keyed):** `reports`, `report_identifiers`, `records`, `record_identifiers`, `project_reports` (which also holds graph metrics and `lifecycle_status`), `citations` (UUID edges), `acquisition_runs` (shares the same id as `ingestions` for DOI traversal), `studies`/`study_reports`, `protocol_versions`/`eligibility_criteria`/`exclusion_reasons`, `screening_events` → `screening_state` projection, `review_events`, `documents`/`document_blocks`/…, `dedupe_proposals`, extraction/appraisal tables, and the AI tables (`ai_runs`, `ai_proposals`, `review_run_manifests`, `review_step_attempts`, calibration bundles, `ai_screening_cohorts`/`dispositions`/`audit_labels`, …). Automation and workflow tables are `automation_*`, `workflow_*`, `assistant_*`.
- **Queue:** one `jobs` table. Kinds: `work_fetch_requested`, `recompute_metrics`, `run_deduplication`, `pmid_import`, `automation_run`, `assistant_agent_run`, `workflow_step`, `retrieve_document`, `parse_document`, and `recompute_prisma`, which the processor acknowledges as a no-op.

Each `work_fetch_requested` job is the old NATS consumer protocol running inside a PG job. That gives **three stacked leases**: the job lease (`jobs.lease_owner`, a process-wide string), the event lease (`processed_events.owner_token`, a fresh UUID per delivery), and the DOI lease (`doi_fetch_state.owner_token`). The job's `attempts` field doubles as the old "delivery_count" (`processor.rs:779`).

### Job lifecycle state machine (`crates/app/src/jobs.rs`)
```
enqueue (INSERT … ON CONFLICT (dedupe_key) DO UPDATE SET id=jobs.id → returns EXISTING row, whatever its state)
   │
 queued ──claim (SKIP LOCKED, attempts+=1, lease_owner=<process string>, leased_until)──► running
   ▲                                                                                      │ renew every lease/3 (owner match + not yet expired)
   │◄── recover_expired_jobs (run before EVERY claim + every reconciler tick):             │
   │      running ∧ leased_until<now → queued   (no attempts check, no backoff)            │
   │◄── fail_job (attempts<max) available_at=now()+delay ◄─────── Err / Nak(delay) ───────┤
 dead ◄── fail_job (attempts>=max) ◄────────────────────────────── Terminate ──────────────┤
 completed ◄── complete_job (owner match ∧ leased_until>now) ◄──── Ack ───────────────────┘
 'failed' is allowed by the CHECK constraint but nothing ever writes it. Nothing ever deletes or purges rows.
```
The fencing token is `lease_owner = "deepref-worker-<uuid per process>"` (`services/worker/src/lib.rs:92`). The attempt number is never part of the predicate on `jobs`. Automation steps add their own `claimed_job_attempt` fence (`automations.rs:526-597`). Workflow steps have no fence at all.

### Background loops (each runs once per worker process; no leader election)
Worker claim loop (one claim per 100 ms tick); reconciler every 30 s (lease recovery, `find_missing_ingestion_work`, `sweep_ai_first`, `sweep_second_reviews`); workflow scheduler every 20 s (`claim_due_workflows` with SKIP LOCKED, `fail_abandoned_steps`).

---

## 2. What is good and should be preserved

- **Transactional outbox-free design.** Most commands write state, follow-up job and audit row in one transaction: `finalize_success` in `store.rs:261`, `enqueue_parse` in `complete_document_retrieval`, `assistant_runs.rs:238`, `pmid_import.rs:134`, and workflow planning with `advance_run_in_transaction`. Dispatching automation domain events inside the caller's transaction is also correct.
- **Ingestion item fencing.** `validate_ownership` re-checks both the event lease and the DOI lease inside the commit transaction (`store.rs:398-432`), and `complete_item_and_claim` fails the transaction when the event claim was lost (`store.rs:988-994`). This is real fencing. The job layer should be brought up to the same standard.
- **Screening command** (`screening.rs:164-279`). It uses a per-report advisory lock, `FOR UPDATE` on the state row, an expected-revision check that returns a 409 carrying the current snapshot, and writes the event, the state projection and the lifecycle status in one transaction. This is the best-modelled aggregate in the codebase.
- **The PRISMA projection is a single live statement** over canonical tables (`prisma.rs:17-25`). There is no snapshot that can drift.
- **The workflow planner is idempotent:** a row lock on the run, deterministic `(run_id,node_id,iteration)` uniqueness, and `ON CONFLICT DO NOTHING` for both nodes and jobs.
- **Automation steps** fence on `(claimed_by, claimed_job_attempt)` and compare it to the job's current attempt (`automations.rs:680-695`). This is the pattern the whole queue should use.
- **The DB enforces several invariants:** a partial unique index for the current extraction value (`0025:45`), composite FKs that prevent cross-project evidence, and CHECK-constrained step states (`0019:165-179`).
- **Process:** migrations are append-only and immutable, `serve` never migrates (ADR 0001), the `.sqlx` cache is checked against a live DB in CI, and `xtask boundaries` enforces the dependency direction.

---

## 3. Findings (ranked by leverage)

### F1. DB-backed integration tests never execute in CI: the test step has no `DATABASE_URL` and every DB test returns `Ok(())` when it is missing
- **Category:** Defect (verification gap). **Severity:** P1. **Confidence:** Very high. **Status:** Verified (config and code reading).
- **Evidence:**
  - In `.github/workflows/ci.yml:563-581`, `DATABASE_URL` is set only as step-level `env` on "Apply database migrations" (l.568) and "Verify SQLx offline query metadata" (l.577).
  - "Run integration tests" (l.579-581) runs `cargo nextest run --workspace --tests --locked` with no `env`. There is no job-level or workflow-level `DATABASE_URL` (workflow `env:` at l.15-18 holds only cargo variables), and nothing writes it to `$GITHUB_ENV`.
  - `.cargo/config.toml` and `.config/nextest.toml` set no env. `.env` is untracked, and test binaries do not load dotenv.
  - Every DB test starts like `crates/app/tests/jobs.rs:18-19`: `let url = std::env::var("DATABASE_URL").ok()?;`, then `let Some(pool) = database().await else { return Ok(()); };` (l.64-66). The same pattern appears in all ~35 files under `crates/app/tests` and in `services/worker/tests`.
  - The previous CI version (`13024c8^`) had the same omission, so this has likely never worked since the full-stack job was split out.
- **Current behavior:** CI reports green while every persistence, queue, screening, AI-first, migration and worker integration test is skipped.
- **Why it matters:** All the durability and concurrency claims in `docs/architecture.md` and ADR 0005 rest on these tests. Today they are checked only on developer machines that have `.env` loaded, against a shared dev DB that some tests mutate (they "park" other jobs as `dead`, e.g. `services/worker/tests/workflows.rs:36-45`).
- **Root cause:** The skip-if-absent convention is meant to keep unit runs fast, but it has no "required" switch.
- **Proposed direction:**
  1. Put `DATABASE_URL` on the job (or on the test step).
  2. Add `DEEPREF_REQUIRE_DATABASE=1` in CI and make `database()` panic when it is set and the URL is missing.
  3. Consider `#[sqlx::test]` or a per-binary template database, so tests stop sharing and mutating one DB.
- **Verification:** Add one test that asserts `DATABASE_URL` is present when `CI=true`. Then compare nextest timing before and after: the DB suites should go from milliseconds to seconds.

### F2. The AI-first safety check turns any failure to compute identity into a committed, destructive demotion that reopens finalized exclusions; it runs every 30 s on finalized cohorts
- **Category:** Defect. **Severity:** P1. **Confidence:** High for the error conflation. Medium-high that reopening finalized cohorts on an identity change is unintended (it diverges from the ADR wording).
- **Status:** Verified by code reading. The trigger scenarios are a strong inference.
- **Evidence:**
  - `crates/app/src/ai_first/routing.rs:35-47`:
    ```rust
    crate::preview_screening_identity(pool, project, report, …).await
        .ok()
        .is_some_and(|contract| contract.id().as_str() == c.get::<String,_>("semantic_bundle_hash"))
    ```
    When this is false, it calls `invalidate_in_transaction(tx, …, "semantic_identity_changed", true)` (l.48-61).
  - `invalidate_in_transaction` (`cohorts.rs:283-311`) does four things: `reopen_finalized` (writes undo screening events for every still-canonical automated exclusion, `cohorts.rs:356-386`); sets the cohort to `invalidated` even from `finalized` (l.291); voids dispositions; and suspends the project's authority.
  - Callers commit after a false result. Examples: `finalize_ai_first_cohort` (`audit.rs:478-483`: `tx.commit()` and then `Refused`) and `sweep_ai_first`. The sweep (`routing.rs:261-275`) selects cohorts in `('open','closed','auditing','passed','finalized')`, and the reconciler runs it on every worker every 30 s (`services/worker/src/second_review.rs:7`).
  - `preview_screening_identity` (`review_preparation.rs:125-144`) uses **a second pool connection** while the caller's transaction holds `projects … FOR UPDATE`, `LOCK TABLE ai_model_routes IN SHARE MODE`, and `FOR SHARE` on member reports. It can fail for reasons unrelated to identity:
    - no published protocol;
    - the first member report (`ORDER BY report_id LIMIT 1`) is missing or no longer a target;
    - `PostgresAiStore::resolve` finds no route, e.g. a worker started without an AI key, since `build_ai_gateway` only ensures routes when a key exists;
    - `runtime_identity()` errors;
    - a pool acquire timeout. Each sweep holds one connection and asks for another, so N concurrent sweeps or finalizations can exhaust the pool.
  - ADR 0005 §2.4 says an identity change should make "open cohorts close as invalidated; their dispositions are voided", and a drift signal leaves "finalized cohorts … flagged for review". Only §13 (protocol republish) reopens finalized exclusions.
- **Current behavior:** A transient DB or pool error, a missing AI key on one worker, or a deploy that changes a prompt asset will, within one reconciler tick, write undo events against previously approved, audited AI-first exclusions across all projects and suspend AI-first everywhere. An audited cohort costs two blinded human labels per sampled record, and this throws it away.
- **Why it matters:** This is irreversible scientific state churn caused by infrastructure noise. It is the opposite of "fail closed": failing closed should mean refusing new automation, not undoing approved work.
- **Root cause:**
  - `Result` is collapsed into `bool` (`.ok()`).
  - One function, `invalidate_in_transaction`, serves every demotion trigger regardless of the cohort's status.
  - An identity read runs on a different connection inside a locking transaction.
- **Proposed direction:**
  1. Make `ensure_current` return a three-way result: `Current | Stale(reason) | Unknown(error)`. On `Unknown`, roll back and log; never invalidate.
  2. Split demotion by trigger and status according to the ADR: identity change voids open or closed cohorts; protocol republish reopens finalized exclusions; finalized cohorts get only a "flagged" marker on identity drift.
  3. Compute the identity from the transaction's connection, or compute it before `BEGIN` and compare under the lock.
  4. Exclude `finalized` from the periodic sweep unless the protocol changed.
- **Verification:** Write a DB test that makes `preview_screening_identity` fail (delete the default route, or drop the sample report's `project_reports` row), run `sweep_ai_first`, and assert that the finalized cohort and its exclusions are unchanged. Then run the same test with a changed prompt hash and assert the result follows ADR §2.4.

### F3. Job fencing uses a process-wide owner string with no attempt token; workflow steps have no fence at all, so a stale execution can complete, overwrite or duplicate side effects
- **Category:** Defect. **Severity:** P2. **Confidence:** High (mechanism). **Status:** Code verified. The race scenarios are a strong inference; I could not run them.
- **Evidence:**
  - The owner string is created once per process (`lib.rs:92`). `renew_job`, `complete_job` and `fail_job` (`jobs.rs:119-183`) match only `id, state='running', lease_owner=$owner, leased_until>now()`.
  - `claim_job` calls `recover_expired_jobs` before every claim (`jobs.rs:76`), so the **same process** reclaims its own expired job, and the old attempt's `complete_job` still matches. Two attempts can then hold the same lease and either can complete it.
  - A transient DB error during renewal (`lib.rs:294`, `renew_job(...).await?`) aborts `run_with_lease_renewal`, which **drops the in-flight work future** mid-execution. That cancels it after possible external effects, and the job is retried.
  - Workflow steps (`services/worker/src/workflows.rs:107-140`, `crates/app/src/workflows.rs:1338-1460, 1519-1620`):
    - `begin_node_execution` accepts nodes already in `running` and bumps `attempts` with no owner recorded.
    - `finish_node_execution` writes `completed`/`failed`/`skipped` without checking the node's current status or attempt, so a late duplicate can overwrite the result of `fail_abandoned_steps` or of a parallel execution.
    - Workflow nodes include outbound HTTP, email and notifications, and review-state changes (via `AUTONOMY_GATE`).
  - Contrast: automation steps fence correctly on `claimed_job_attempt` (`automations.rs:526-597, 680-695`), and review-run writes join to `jobs.lease_owner` (`review_runs.rs:349, 382, 791`), though they too omit the attempt.
- **Current behavior:** At-least-once delivery with weak fencing. Duplicates need a lease expiry, which is uncommon at the 60 s lease with renewal every 20 s. But a renewal failure deliberately causes one, and the reconciler's recovery is aggressive.
- **Why it matters:** Workflow side effects (webhooks, emails, screening writes) and paid AI calls can run twice. A stale attempt can also mark a job `completed` while the live attempt is still running.
- **Root cause:** The lease token identifies the process, not the claim.
- **Proposed direction:**
  1. Return a `claim_token` (or `attempts`) from `claim_job` and require `AND attempts=$n` (or `lease_token=$t`) in renew, complete and fail. Thread it through as a `JobLease` value rather than `&str owner`.
  2. In workflows, store `claimed_attempt` on `workflow_node_runs`. Make begin and finish compare-and-set on it, and make finish a no-op unless `status='running'`.
  3. On a renewal error, retry with backoff before giving up the lease. Don't cancel work because of one failed UPDATE.
- **Verification:** Unit-test with a lease of a few ms: claim, let it expire, reclaim from the same owner, then call `complete_job` with the first attempt's token and expect `false`. Also run a workflow test with two concurrent `handle_step` calls on one node and assert one execution and one outcome.

### F4. Lease-expiry recovery skips `max_attempts` and backoff, so a job that crashes or stalls the worker is retried forever and never reaches `dead`
- **Category:** Defect. **Severity:** P2. **Confidence:** High. **Status:** Verified (code).
- **Evidence:**
  - `recover_expired_jobs` (`jobs.rs:59-66`) sets `state='queued'` without checking `attempts` and without moving `available_at`.
  - The `CLAIM` CTE (`jobs.rs:12-28`) has no `attempts < max_attempts` predicate.
  - Only `fail_job` ever moves a job to `dead`, and it never runs if the process dies (OOM, segfault in the PDFium parser, a panic in a blocking task) or the lease is lost.
- **Current behavior:** A poison PDF or payload that kills the worker gets reclaimed immediately after lease expiry, again and again (attempts 6, 7, …). Every worker that picks it up crashes or stalls.
- **Why it matters:** One input can stop a whole queue, and the failure never shows up as a `dead` job.
- **Proposed direction:** In recovery, set `state = CASE WHEN attempts >= max_attempts THEN 'dead' ELSE 'queued' END`, `last_error='lease expired'`, and `available_at = now() + backoff(attempts)`. Also add `attempts < max_attempts` to the claim as a second guard.
- **Verification:** A test that claims a job with `max_attempts=1`, expires the lease, runs recovery, and asserts `dead`.

### F5. Ingestion run status is recomputed by O(N) scans on every item under READ COMMITTED; concurrent completions can leave runs stuck or mislabelled, and cancellation updates only one of the two run tables
- **Category:** Defect. **Severity:** P2. **Confidence:** High for the cancel drift (seen in real data). Medium for the counting race.
- **Status:** Cancel drift verified (code and data). Race is a strong inference from PostgreSQL READ COMMITTED semantics.
- **Evidence:**
  - `complete_item_and_claim` (`store.rs:902-996`) runs on every item. It does `item_outcome_counts` (statement snapshot S1), then `UPDATE ingestions` with six scalar subqueries over `ingestion_items` (snapshot S2, taken *before* it blocks on the `ingestions` row lock held by a concurrent completer), then `UPDATE acquisition_runs` with six more subqueries.
  - Race: completers A and B finish the last two items. A's subqueries see B's item as `fetching`, and B's S2 predates A's commit. Under PG's EvalPlanQual rule only the target row is re-read after the wait; the subqueries keep the old snapshot. Both see one item still in flight, so `ingestions.status` stays `running` with `queued_count=1` and `completed_at=NULL`, and no later event touches the row.
  - `settled` comes from S1, so `acquisition_runs` can be set to `failed` even though A fetched an item. That status drives the `AcquisitionCompleted` automation event and the notification.
  - Cost: about 14 scans of `ingestion_items` per completion, which is O(N²) per run. Depth is unbounded (F9), and depth 2 already means thousands of items.
  - Cancel: `cancel_ingestion` (`crates/http-api/src/routes/ingestions.rs:248-258`) updates only `ingestions`, without a status guard (a completed run can be "cancelled") and without an existence check (returns 202 for any UUID).
  - Real dev-DB drift: run `9ddd4f7e…` is `ingestions.status='cancelled'` and `acquisition_runs.status='completed'`. Run `f034f9bb…` is `failed` in both tables despite 30 fetched items. That is a semantics change in `6fb4acc` that was never backfilled.
  - Dead jobs: when a `work_fetch_requested` job ends `dead` before the item is finalized (e.g. `Busy` naks, or errors after `mark_fetching`), the reconciler resets the item to `queued` (`worker_runtime.rs:78-83`). `find_missing_ingestion_work` only re-enqueues when *no* job row exists (`worker_runtime.rs:96-99`), and the dead row still exists, so the run never settles.
- **Why it matters:** Run status is what the UI, notifications and automations key on. Drift between `ingestions` and `acquisition_runs` makes the legacy and v2 views disagree.
- **Root cause:** Status is a derived aggregate recomputed from scratch by each writer, without serialization; there are two copies of the run row; and recovery compares item status with job existence instead of job state.
- **Proposed direction:**
  1. Lock the run row (`SELECT … FROM acquisition_runs WHERE id=$1 FOR UPDATE`) **before** reading counts. A new statement after the lock sees committed peers.
  2. Better: maintain counters incrementally (`fetched_count = fetched_count + 1`, `queued_count = queued_count - 1`) in the same UPDATE that changes the item, and settle when `queued_count=0`.
  3. Make `acquisition_runs` the only run row and serve `/ingestions` as a view over it. Route cancel through it with `WHERE status IN ('queued','running')` and a 404 for unknown ids.
  4. In the reconciler, treat `j.state IN ('dead','failed')` for a queued item as "item failed", not as missing.
- **Verification:** Run a DB test that completes the last two items in parallel transactions using a barrier (e.g. `pg_advisory_lock` held by the test), and assert the terminal status and counts. Then run `SELECT … FROM acquisition_runs a JOIN ingestions i USING (id) WHERE a.status<>i.status` as a drift query.

### F6. Every ingested DOI writes both the legacy and the v2 model in one hot transaction; the legacy tables are now mostly write-only and the mirrors drift
- **Category:** Architecture. **Severity:** P2. **Confidence:** High. **Status:** Verified.
- **Evidence:**
  - `finalize_success`/`persist_work`/`attach_project_work`/`persist_citation` (`store.rs:261-632`) write, per DOI and per reference: `works`, then `reports` (copied *from* `works` via `ensure_v2_report`, whose report id is an md5 built in SQL), `report_identifiers`, `records`, `project_works`, `project_reports`, `legacy_citations`, `citations`, `fetched_citation_facts`, `unresolved_references` (legacy only, with no v2 equivalent), `fetched_unresolved_reference_facts`, and a `domain_events` row for every citation and every unresolved reference.
  - `ensure_v2_report` runs up to three times per citation, each doing an upsert of `reports`.
  - Readers (`git ls-files … | xargs grep`):
    - `legacy_citations` is read only by `legacy_import.rs`.
    - `domain_events` is read only for `MAX(revision) … entity_type='metric'` (`graph.rs:206`).
    - `crossref_raw_records` and `prisma_snapshots` have no reader or writer (`prisma.rs:21` says it "is not consulted").
    - `processed_events`/`dead_letter_records` serve only the in-job NATS emulation.
  - Dev DB: 10,604 `domain_events`, 8,397 `legacy_citations` vs 8,419 `citations`, 5,703 `works` vs 5,809 `reports`.
  - ADR 0003 calls this "during DOI compatibility" and "staged rollout".
- **Why it matters:**
  - Write amplification on the hottest path. Roughly 10 tables plus N events per DOI means larger transactions, more lock contention, and more WAL.
  - Two sources of truth for run status (F5), membership (`project_works` vs `project_reports`), and edges.
  - A reader cannot tell which table is canonical. `docs/architecture.md` says graph reads use `citations`, but `unresolved_references` exists only in the legacy shape.
- **Root cause:** An expand phase with no contract phase.
- **Proposed direction:** Run an explicit expand/contract migration plan (see §5). Ingestion should write v2 only: `reports`/`report_identifiers`/`records`/`project_reports`/`citations`, plus a v2 `unresolved_references` keyed by report. Keep `works` only as the raw Crossref cache, or fold it into `records.raw`. Stop emitting `domain_events` from ingestion, and replace the metric "revision" with a project-scoped sequence. Freeze `legacy_*` behind `import-legacy` and drop them one release later.
- **Verification:** Count rows written per DOI before and after (pg_stat_user_tables `n_tup_ins` deltas over one ingestion). Golden graph API responses should stay identical.

### F7. Each ingested DOI enqueues a full-project metric recompute that is never coalesced; it runs at REPEATABLE READ with one UPDATE per report, and 35% of recomputes hit serialization failures
- **Category:** Behavior-preserving (performance) and Defect (staleness). **Severity:** P2. **Confidence:** High. **Status:** Verified (code and dev-DB data).
- **Evidence:**
  - `emit_metrics_request` runs on every item completion (`store.rs:247, 292, 733-764`). Its dedupe key `recompute_metrics:{project}:{event_id}` is unique per item, so nothing is coalesced.
  - `recompute_project_metrics` (`graph.rs:180-277`) loads the **complete** project graph, then issues one `UPDATE project_reports` per node inside a REPEATABLE READ transaction.
  - Dev DB: 225 `recompute_metrics` jobs for 227 `work_fetch_requested` jobs. 78 of them carry `last_error = 'could not serialize access due to concurrent update'`, and max attempts reached 4 (the limit is 5).
  - The fixed 10 s retry (`lib.rs:242-256`) delays freshness. If the *last* recompute of a run goes `dead`, metrics stay stale, because the reconciler does not recompute.
- **Why it matters:** Cost is O(items × reports) row updates per ingestion (160 DOIs × ~5k reports ≈ 800k UPDATEs). Concurrent recomputes are mutually exclusive by design, so most of that work is wasted.
- **Proposed direction:**
  1. Coalesce with one queued recompute per project: a partial unique index `ON jobs(kind, project_id) WHERE state='queued' AND kind='recompute_metrics'`, with enqueue as `ON CONFLICT DO NOTHING`. A running recompute plus at most one queued one is enough.
  2. Write metrics with one `UPDATE … FROM unnest($ids, $metrics…)`.
  3. Have the reconciler requeue a recompute when `projection_state.revision` lags behind the project's citation watermark.
- **Verification:** Ingest the same seed set and compare the count of `recompute_metrics` jobs and the serialization-failure rate. The graph API output should be unchanged.

### F8. `dedupe_key` is unique forever and conflates in-flight deduplication with "once ever"; revival is ad hoc and erases history, and the jobs table is never pruned
- **Category:** Architecture. **Severity:** P3. **Confidence:** High. **Status:** Verified.
- **Evidence:**
  - `jobs.dedupe_key text UNIQUE` (`0006:166`). `enqueue_job` uses `ON CONFLICT (dedupe_key) DO UPDATE SET id=jobs.id RETURNING id` (`jobs.rs:30-51`). It silently returns a completed or dead job's id, without checking that `kind` or `project_id` match. Only the SQL automation function checks that (`0019:469-478`).
  - Documents work around it by rewriting a terminal job back to `queued, attempts=0, last_error=NULL, completed_at=NULL` (`documents.rs:500-506`), which erases the failure history. Reparse invents a per-request key instead (`documents.rs:371-393`).
  - `enqueue_parse` ignores its `_content_hash` parameter (it has since the parameter was introduced in `6745b1e`).
  - No `DELETE FROM jobs` exists anywhere. `jobs_dedupe_kind_idx (kind, dedupe_key)` (`0008:60`) duplicates the unique index.
  - `fail_abandoned_steps` scans all dead `workflow_step` jobs on every scheduler tick by `payload->>'node_run_id'` (`workflows.rs:1650-1656`), which has no index.
- **Why it matters:** Every new caller has to know that a stable key will silently do nothing after completion. Revival destroys the audit trail. Table growth slows recovery and diagnostic scans.
- **Proposed direction:** Use separate concepts:
  - an **active-dedupe** partial unique index `(dedupe_key) WHERE state IN ('queued','running')`;
  - an optional **idempotency key** for genuinely once-only work;
  - a `job_attempts` history table, or retention of terminal rows past N days with an archive.
  
  Return a typed `Enqueued::{Created, AlreadyActive(id), AlreadyDone(id)}`.
- **Verification:** Run the existing job tests (once F1 is fixed) plus a test that enqueueing after completion with the same key creates a new row.

### F9. Citation traversal depth has no upper bound
- **Category:** Product/Defect. **Severity:** P2. **Confidence:** Very high. **Status:** Verified.
- **Evidence:** `acquisitions.rs:183-206` rejects only `< 0`. `settings.rs:82` only rejects negative values. There is no CHECK on `ingestions.max_depth`, `projects.default_max_depth` or `settings.default_max_depth` (0001, 0007, 0048). Migration 0048's own comment says "Depth 2 can add thousands of records".
- **Why it matters:** Fan-out is exponential, and it combines with the quadratic per-item status work (F5) and the per-item full recompute (F7). One request can pin the worker pool and the Crossref rate budget for days.
- **Proposed direction:** Enforce a domain-level `MaxDepth` (e.g. ≤ 3) in `deepref-domain`, add DB CHECKs, and add a per-run item cap with an explicit "truncated" status.
- **Verification:** Run the API test with depth 10 and expect 400.

### F10. Every screening decision takes `projects … FOR UPDATE`, a project-wide mutex that also blocks FK inserts into every project child table
- **Category:** Architecture/Behavior-preserving. **Severity:** P3. **Confidence:** Medium. **Status:** Strong inference.
- **Evidence:** `screening.rs:169` and `286`, `ai_first/cohorts.rs:47`, `protocol.rs` (×2), `ai/proposals.rs`, `ai_reviewer.rs`, `review_runs.rs`: 8 sites. All but the AI-first one were added in `fc4ea73` (2026-10-09).
  - `FOR UPDATE` conflicts with the `FOR KEY SHARE` lock that FK checks take. So while any screening transaction is open, inserts into `jobs`, `records`, `notifications`, `review_events`, `screening_events`, and any other table with a `project_id` FK to `projects` wait on it.
  - Lock order mixes the project row, advisory locks (`screening:{p}:{r}`), cohort rows, `LOCK TABLE ai_model_routes IN SHARE MODE`, and member `FOR SHARE`. Deadlocks are possible across these paths, and they surface as HTTP 500 (F11).
- **Why it matters:** Screening throughput per project, and background work such as imports and job enqueue, now serialize on one row. The intent (serializing against cohort finalization and protocol publish) is valid; the lock mode is stronger than that intent needs.
- **Proposed direction:** Use `FOR NO KEY UPDATE`, which does not block FK checks, or a project-scoped advisory lock such as `ai_first:{project}` that only the AI-first and protocol paths share. Document one global lock order.
- **Verification:** Run `pgbench`-style concurrent screening plus imports on one project and watch `pg_locks` waits before and after.

### F11. Persistence errors collapse to 500; SQLSTATE handling is ad hoc and nothing retries serialization failures or deadlocks
- **Category:** Architecture. **Severity:** P3. **Confidence:** High. **Status:** Verified.
- **Evidence:** In `crates/http-api/src/error.rs:62-82`, `Database(RowNotFound)` maps to 404 and every other `sqlx::Error` to 500 `INTERNAL_ERROR`. SQLSTATE checks are hand-rolled in five modules (`extraction.rs:915`, `workflows.rs:216`, `assistant*.rs`, `automations.rs:609`, `documents.rs:1088`). There is no handling of `40001`/`40P01` anywhere, even though `recompute_project_metrics` deliberately uses REPEATABLE READ.
- **Why it matters:** Clients cannot tell a retryable conflict (409/503 with Retry-After) from a bug. Racing deletes (23503) and unique races (23505) show up as 500s outside the five handled modules.
- **Proposed direction:** Add a single `classify(sqlx::Error) -> PersistenceError {NotFound, Conflict(constraint), Retryable, Integrity, Other}` in `deepref-app`, a `with_retry` helper for serializable transactions, and an `ApiError` mapping from it.
- **Verification:** Unit-test `classify` against fabricated `PgDatabaseError` codes.

### F12. The persistence layer is mostly runtime-checked SQL with stringly-typed states; the typed IDs and the `.sqlx` cache cover a small fraction
- **Category:** Architecture. **Severity:** Opportunity. **Confidence:** High. **Status:** Verified.
- **Evidence:**
  - About 1,661 runtime `sqlx::query(…)`/`query_scalar(…)`/`query_as(…)` calls vs 33 compile-checked macros (29 `.sqlx` files).
  - States are compared as strings in Rust (`c.get::<String,_>("status") != "passed"`, `audit.rs:473`; `jobs` states; document status; cohort status).
  - `deepref-domain` defines only `ProjectId`, `ProtocolVersionId` and `ExclusionReasonId` via `typed_id!` (`lib.rs:29-61`), with free `From<Uuid>`. `project_id: Uuid` appears 289 times vs `ProjectId` 178 times in postgres/http-api/worker.
  - Business rules are layered through renamed plpgsql functions (`*_legacy` wrappers in 0054/0055), with 38 SQL functions and 24 triggers.
- **Why it matters:** Schema drift is caught only at runtime, and F1 means that is not in CI. State machines (job, document, cohort, workflow node) cannot be checked for exhaustiveness.
- **Proposed direction:** Add enum mappings (`#[derive(sqlx::Type)]`) for every state column, newtypes for `ReportId`/`JobId`/`CohortId`, and move hot or complex queries to `query!`/`query_as!` incrementally, starting with the jobs and screening modules.

### F13. Optimistic concurrency covers only screening, protocol and workflow drafts; other mutable aggregates are last-writer-wins
- **Category:** Product/Architecture. **Severity:** P3. **Confidence:** Medium-high. **Status:** Verified for extraction and appraisal.
- **Evidence:** `record_manual_value` (`extraction.rs:310-450`) supersedes the current value and inserts a new one with no expected version. The partial unique index (`0025:45-47`) turns a truly concurrent double-write into a unique violation, which is mapped to `extraction_conflict`, but a stale-view overwrite succeeds silently. Appraisal completion and study rename/classify also take no revision. The 409 codes that exist are `screening_revision_conflict`, `protocol_conflict`, `extraction_conflict` and `review_*_conflict`.
- **Why it matters:** Two reviewers editing extraction or appraisal for the same study lose updates without notice. Screening shows the team already values this protection.
- **Proposed direction:** Add a `revision bigint` (or use the current row id) as `expected_*` on extraction values, appraisal assessments and studies, and reuse the screening 409 contract.

### F14. Background sweeps run on every worker over every project every 30 s, claim and schedule non-atomically, and check budget before spending
- **Category:** Defect (minor) / Architecture. **Severity:** P3. **Confidence:** Medium. **Status:** Verified (code); impact inferred.
- **Evidence:**
  - `sweep_second_reviews` (`second_review.rs:133-240`) iterates `SELECT id FROM projects` for all projects. It commits an `INSERT second_review_attempts` claim, *then* schedules in a separate transaction. A crash between the two leaves a permanent attempt with no run, and the comment says such records are never retried.
  - The budget check is `spent >= budget` before scheduling, which is check-then-act across workers.
  - `sweep_ai_first` takes a project lock and a SHARE table lock on `ai_model_routes` for each cohort, on every worker.
- **Proposed direction:** Schedule inside the claim transaction (the scheduler already supports transactional enqueue), or turn the sweep into a job kind with a dedupe key per tick per project, so only one worker sweeps a project per tick.

### F15. Queue throughput is capped by the claim loop: one claim per 100 ms tick per worker, preceded by a recovery UPDATE
- **Category:** Opportunity. **Severity:** Opportunity. **Confidence:** High. **Status:** Verified.
- **Evidence:** In `lib.rs:114-150`, `claim_tick` is 100 ms and each tick claims at most one job. `claim_job` always runs `recover_expired_jobs` first (`jobs.rs:76`).
- **Why it matters:** Short jobs, such as cached-DOI attaches and the no-op `recompute_prisma`, top out at about 10 per second per worker regardless of `WORKER_CONCURRENCY`, and every tick does a write.
- **Proposed direction:** Claim in batches up to the free permits (`LIMIT $n`), use `LISTEN/NOTIFY` or an adaptive tick, and leave recovery to the reconciler.

---

## 4. Deletion candidates

| Candidate | Evidence | Notes |
|---|---|---|
| `recompute_prisma` job kind | `processor.rs:808` acks it as a no-op | Drop the match arm after confirming no queued rows remain |
| `prisma_snapshots` table | 0 rows; `prisma.rs:21` says it "is not consulted" | Drop in a contract migration |
| `crossref_raw_records` table | 0 rows; no Rust reference | Drop |
| `jobs.state='failed'` value | Never written (`fail_job` only writes queued/dead) | Remove from the CHECK, or define it; `fail_abandoned_steps` and the docs filter on it |
| `jobs_dedupe_kind_idx` | Duplicates the `dedupe_key` UNIQUE index | Drop |
| `processed_events` + `dead_letter_records` + `deepref_events` delivery vocabulary (`DeliveryAction::Nak`, `MAX_DELIVERIES`, `delivery_count`) | Emulate NATS inside a PG job | Fold into the job lease and `jobs.last_error`; keep `doi_fetch_state` only as a cross-ingestion fetch lock |
| `domain_events` writes from ingestion (`store.rs:766-795`), `domain_tombstones` | Only reader is `MAX(revision)` for metric freshness | Replace with a per-project sequence or watermark column |
| `legacy_citations` dual write; `project_works`, `unresolved_references` (legacy shape) | Read only by `legacy_import` | Contract after a v2 equivalent exists |
| `ingestions` as a separate row | Duplicates `acquisition_runs` (same id), and they drift | Turn it into a view |
| Five `handle_job_with_*` wrapper variants (`processor.rs:268-366`) | Only differ in which `Option` services are set | One `JobServices` builder |
| Empty untracked `services/api/migrations/` directory | Leftover from before ADR 0002 | `rm -r` locally |
| `*_legacy` SQL functions chained by rename (0054/0055) | Logic split across wrapper layers | Inline into one function per rule in a later migration |

---

## 5. Zero-based redesign thoughts

### 5.1 Job queue: three options
- **A. Keep the PG queue, harden it (recommended, small).**
  - Use `claim_token uuid` per claim, required in renew, complete and fail.
  - Make recovery respect `max_attempts` and backoff.
  - Separate the active-dedupe partial unique index from the idempotency key.
  - Add a `job_attempts` history table and retention for terminal rows.
  - Do batch claims and `LISTEN/NOTIFY` wakeups.
  - Give handlers the job's transaction, or a fenced commit helper (`commit_if_lease_held(tx, lease)` that runs `SELECT … FROM jobs WHERE id=$1 AND claim_token=$2 FOR SHARE` before commit), so *every* handler's state change is fenced the way ingestion already is.
  - This removes the need for `processed_events`.
- **B. Adopt a library queue (e.g. a pgmq-style or `apalis`-style PG backend).** You get attempt fencing, visibility timeouts and archiving for free. The cost is losing the in-transaction enqueue unless the library supports it, which pgmq does via SQL functions. It is worth it only if the team wants to stop owning queue code.
- **C. Per-aggregate state machines plus a thin scheduler.** Each long-running aggregate (`acquisition_runs`, `review_run_manifests`, `workflow_node_runs`, `documents`) owns its status, `attempt` and `lease_token`, and the generic `jobs` row only wakes things up. This is closest to what automation steps already do. It makes fencing explicit per aggregate, but it spreads the lease logic around unless it is factored into one helper.

### 5.2 Ingestion: one model
Run expand/contract in three releases:
1. Introduce `report_fetch_state` (replacing `doi_fetch_state` and `works.fetch_status`), a v2 `unresolved_references(project_id, source_report_id, …)`, and counters on `acquisition_runs`. Write both.
2. Switch the readers (`/ingestions` becomes a view on `acquisition_runs`; graph reads already use v2) and stop the legacy writes.
3. Drop `legacy_citations`, `project_works`, `ingestion_items` (folded into an `acquisition_items` table keyed by report), `processed_events`, `dead_letter_records`, and `domain_events`/`domain_tombstones`.

The per-DOI transaction shrinks to about 5 tables. Run status becomes an incremental counter, which also fixes F5.

### 5.3 AI-first demotion as an explicit, pure state machine
Model `CohortState × Trigger → Effect` in `deepref-application` (pure and table-tested from ADR 0005 §2.4 and §13). Triggers are `IdentityStale(reason)`, `ProtocolRepublished`, `AuditFailed`, `LabelExposed`, `Unknown(error)`. The effects are explicit: `VoidOpen`, `Reopen(finalized)`, `Flag`, `Suspend`, `NoOp`. The Postgres adapter only applies effects. This makes the F2 conflation impossible to write and keeps the ADR and the code in lockstep.

---

## 6. Open questions

1. **Is reopening finalized AI-first exclusions on a non-protocol identity change intended?** The ADR text says no; the code does it. There may be an unwritten V1 decision. Ask the owner.
2. **Can I run the experiments?** I could not create an isolated scratch DB (denied). Suggested commands for the owner:
   - Create a throwaway DB (`createdb review_scratch`), apply the migrations, then run `DATABASE_URL=… cargo nextest run -p deepref-app -p deepref-worker` to see whether the suites pass (F1).
   - Reproduce F5 with two `psql` sessions completing the last two items around a held advisory lock.
   - Reproduce F3 with a 50 ms lease and the same owner string.
3. **Are any deployments running more than one worker process?** Several sweeps (F14) and the project-row locks (F10) assume low concurrency.
4. **Is the `domain_events` revision used externally** (`/projection` freshness clients)? If not, it can be replaced (F6, §4).
5. **Migration numbering gaps** (0032–0039, 0043–0044, 0046–0047) came in as one squash (`6fb4acc`). No branch currently has colliding numbers. Is a numbering-reservation convention intended? sqlx applies lower-numbered late-arriving migrations out of order without complaint, so a CI check for "new migration number > max on main" would make the convention explicit.
6. **Is `works` (the global Crossref cache) meant to stay authoritative for report metadata?** `ensure_v2_report` overwrites `reports.title`/`abstract` from `works` on every re-fetch, which would clobber any curated v2 metadata edits. I did not trace whether such edits exist.
