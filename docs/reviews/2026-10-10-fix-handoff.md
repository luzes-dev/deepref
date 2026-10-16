# Fix handoff: whole-application review, 2026-10-10

You are applying the fixes from the DeepRef whole-application review. This file is the work order:

- a list of ordered **packets**;
- what each one fixes, where, how, and the test that proves it;
- a "done when" line for each.

Background:

- The full reasoning is in [`2026-10-10-whole-application-review.md`](2026-10-10-whole-application-review.md). Section letters (§D, §S…) in this file refer to it.
- Raw reviewer reports are in [`2026-10-10-evidence/`](2026-10-10-evidence/). Codes like "data F2" mean finding F2 in `1-data-jobs.md`, and so on.

| Code | File                  |
| ---- | --------------------- |
| data | `1-data-jobs.md`      |
| sec  | `2-security.md`       |
| arch | `3-architecture.md`   |
| fe   | `4-frontend-ux.md`    |
| ai   | `5-ai.md`             |
| ops  | `6-testing-ops-dx.md` |
| prod | `7-product.md`        |

Open the matching evidence section when a packet needs more context than this file gives. Each section has the full trace, root cause and alternatives.

The review cites line numbers from commit `b0ad219` (main, 2026-10-10). Re-locate code by symbol name if lines have moved.

---

## How to work

1. **Work packets in order.**
   - Packet 1 is a prerequisite for trusting any DB-backed test you write later.
   - After packet 1, packets 2–6 are independent. Do them in the listed order unless the user redirects.
2. **Prove each bug red first.** Write the regression test named in the packet, run it, and watch it fail for the reason the packet describes. Then fix it and watch it go green. A test that never went red has proven nothing.
   - The verification column in review §U lists the intended proof for each change.
3. **One packet = one branch = one reviewable change.**
   - Name the branch after the packet's commit title.
   - Keep mechanical deletions out of fix packets.
   - Commit when a packet is done. Pushing and opening PRs is the user's call: ask before doing either.
4. **Stop at decision gates.** Some recommendations need an owner decision; they are listed under [Decision gates](#decision-gates-ask-the-owner-first). When a packet's best fix depends on one, implement the gate-free part, then ask.
5. **Re-verify before trusting.** Every finding here was verified by code reading at `b0ad219`, and the top five were re-checked by hand. Still, read the cited code first. If a finding no longer holds, mark it `no_change_needed` in your report and move on.

### Verification gates (run before calling any packet done)

- **Web (any `.svelte`/`.ts`/`.css` change):** follow `AGENTS.md` §3 exactly: `pnpm check`, `pnpm test`, `pnpm lint`, then Prettier on modified files. Design-system rules in `AGENTS.md` §1–2 apply to every UI change.
- **Rust:**
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`
  - `cargo nextest run --workspace --lib --bins --locked`
  - `cargo xtask boundaries`
- **Migrations or SQL:**
  - run the DB suite against a throwaway database (see the next section);
  - run `just sqlx-prepare` if you change a `query!` macro, and commit the `.sqlx` diff.
- **API shape changes:** `just codegen`, then commit the regenerated web client (`just codegen-check` must pass).

**Baseline at `b0ad219`:** all of the following were green.

- pnpm check: 0 errors.
- pnpm lint: 0 errors, 63 `shadcn/no-restyle` warnings. Don't add warnings.
- Web tests: 535. UI tests: 26.
- clippy: clean.
- Rust unit tests: 569 passed, 2 ignored.
- cargo-deny: ok.

The logs are in `2026-10-10-evidence/baseline/`.

### Environment gotchas the repo won't tell you

- **DB tests skip silently** when `DATABASE_URL` is unset. They return `Ok(())` and report PASS in about 10 ms, so a DB test that passes in milliseconds did not run. Check the duration.
- **Point DB tests at a throwaway database, never the dev DB.** `just test-integration` uses the dev database, and the tests claim and "park" real jobs (`services/worker/tests/workflows.rs:36-45`). The review used a disposable container:
  ```bash
  docker run -d --rm --name deepref-fix-db -e POSTGRES_PASSWORD=postgres -e POSTGRES_DB=deepref \
    -p 55439:5432 pgvector/pgvector:0.8.7-pg17-bookworm
  export DATABASE_URL=postgres://postgres:postgres@127.0.0.1:55439/deepref
  cargo run -q -p deepref-server -- migrate
  cargo nextest run --workspace --tests --locked
  ```
  Stop the container when you're done.
- **Expect 19 known interference failures in a parallel DB run until packet 1 lands:** 837/856 pass. The same binaries pass 33/33 when run serially (`--test-threads 1`). The failing tests are listed in packet 1. Any _other_ failure is real.
- **Run Node through mise.** `engines` requires Node ≥ 24, but a bare shell may give Node 22. Use `mise exec -- pnpm …`, or activate mise first.
- **Disk is tight.** `/home` sat at 99% after the review. Untracked worktrees and their `target/` dirs inside the repo use about 64 GB. Before a full Rust build, check `df -h /home`. If it is nearly full, ask the user before deleting anything.
- **The user's shell is fish.** Write `bash -c '…'` for anything using `$(…)`, loops or heredocs.
- **`AGENTS.md` links point at a nonexistent `/home/luzes/.t3/...` path.** Read the repo-relative files instead, e.g. `packages/ui/src/styles/styles.css`.

---

## Packets

Severity uses the review's scale: P1 is high, P2 medium, P3 low. Packets 1–6 are review roadmap Stage 0 (days). Packets 7–12 are Stage 1 (weeks).

### Packet 1: `ci: run DB integration tests with isolated databases` (P1, do first, alone)

**Problem.** About 30k lines of DB-backed tests never run in CI:

- The CI test step has no `DATABASE_URL` (`.github/workflows/ci.yml:563-581`; it is set only on the migrate and sqlx-check steps).
- Every test helper does `std::env::var("DATABASE_URL").ok()?` and returns `Ok(())` (example: `crates/app/tests/jobs.rs:18-19,64-66`; the same pattern is in all ~35 files under `crates/app/tests`, `services/worker/tests` and `crates/http-api/tests`).
- When the tests do run in parallel, 19 fail from shared-DB interference. `DATABASE_TEST_MUTEX` is in-process only, and tests claim jobs globally.

Evidence: data F1, ops F1.

**Fix.**

1. Set `DATABASE_URL` at job level in the `full-stack` job.
2. Add a required-DB switch: with `DEEPREF_REQUIRE_DATABASE=1` (set in CI), the shared `database()` helpers `panic!` when the URL is missing instead of returning `None`.
   - Prefer one shared helper over ~35 copies if a test-support module fits the crate layout.
3. Isolate tests:
   - **Preferred:** a fresh database per test binary or per test, via `#[sqlx::test]` or by cloning a migrated template database.
   - **Fallback:** a nextest test-group with `max-threads = 1` for the interfering binaries. That is cheaper, but slower and hides the coupling.
4. Use the existing, unused `--profile ci` from `.config/nextest.toml`.
5. Remove the duplicate `--tests` run: `rust/test` and `full-stack` run the same binaries today (ops F11).

**Known parallel failures** (they should all pass once tests are isolated):

- `deepref-http-api::assistant_postgres` ×2;
- `deepref-app::ai_first` ×3: `a_new_positive_control_…`, `audit_failure_recovers_…`, `blinded_fixed_audit_…`;
- `deepref-app::jobs` ×2: `enqueue_returns_canonical_id_…`, `expired_leases_are_fenced_…`;
- `deepref-worker::ai_first` ×1;
- `deepref-worker::assistant` ×7;
- `deepref-worker::workflows` ×4.

**Red proof.** Temporarily invert `leased_until > now()` in `complete_job` (`crates/app/src/jobs.rs`). CI must go red on `jobs.rs` tests. Revert the inversion.

**Done when:**

- In CI, the DB suites take seconds, not milliseconds.
- The deliberate inversion fails CI.
- A full parallel run passes 856/856 (or more) with no serial-only workarounds left unexplained.
- Missing `DATABASE_URL` under `DEEPREF_REQUIRE_DATABASE=1` panics.

### Packet 2: `fix(web): guard screening shortcuts against repeat and re-entrancy` (P1)

**Problem.** Holding or quickly re-pressing I/E/M records decisions on reports the reviewer never saw.

- In `apps/web/src/lib/features/screening/components/ScreeningFocus.svelte`, `decide()` (≈312-369) guards only on `decisionMutation.isPending`. That flag is false across `await updateUrl(...)` (≈332), which runs before `mutateAsync` (≈334), and across `await invalidateScreeningReport()`, which runs after it.
- `handleKeydown` (≈461-475) never checks `event.repeat`.
- `current = $derived(queueItems[currentIndex] ?? queueItems[0] ?? null)` (≈124) silently falls back to the _first_ queue item while the URL names a just-removed report.
- `FullTextScreening.svelte:338-363` has the same keydown pattern.

Evidence: fe F1.

**Fix.**

1. Ignore `event.repeat` for decision keys in both screens. Arrow keys may keep auto-repeat.
2. Set a synchronous `deciding` flag on the first line of `decide()` and clear it only after navigation and the mutation both settle.
3. Capture `reportId` before the first `await`.
4. When `urlState.report` is set but missing from the queue, use the precomputed next item instead of falling back to `queueItems[0]`.

**Red proof.** A component test dispatches 5 `keydown` events (one plain, then `repeat: true`) and a double press in the same tick. Assert exactly one decision POST, and that it targets the displayed report.

**Done when:**

- The red test passes for both screens.
- Undo still works.
- The AGENTS.md web gates are green.

### Packet 3: `fix(ai-first): never demote on unknown identity; keep finalized cohorts` (P1)

**Problem.** Any failure to compute semantic identity becomes a committed, destructive demotion.

- `crates/app/src/ai_first/routing.rs:35-61` does `preview_screening_identity(...).await.ok().is_some_and(...)`. The `.ok()` turns a failure into "false", and on false it calls `invalidate_in_transaction(..., "semantic_identity_changed", true)`.
- `invalidate_in_transaction` (`ai_first/cohorts.rs:283-386`):
  - reopens _finalized_, human-audited exclusions by writing undo screening events;
  - voids dispositions;
  - suspends AI-first.
- The sweep (`routing.rs:261-275`) includes cohorts with status `finalized`. It runs on every worker every 30 s (`services/worker/src/second_review.rs:7`).
- Failure sources include:
  - pool exhaustion: the identity is read on a _second_ pool connection while the caller's transaction holds locks (`review_preparation.rs:125-144`);
  - a worker started without an AI key, so no route exists;
  - a missing sample report.
- ADR 0005 §2.4 says an identity change voids _open/closed_ cohorts and only _flags_ finalized ones. Only §13 (protocol republish) reopens finalized exclusions.

Evidence: data F2.

**Fix.**

1. Make the identity check return `Current | Stale(reason) | Unknown(error)`. On `Unknown`: roll back, `warn!` with the cause, and change nothing.
2. Split demotion by trigger and cohort status, following ADR 0005:
   - identity drift voids `open`/`closed`/`auditing`/`passed` cohorts and only flags `finalized` ones;
   - protocol republish keeps today's reopen behavior.
   - If the schema has no "flagged" marker, add a column through a migration.
3. Compute the identity on the transaction's own connection, or compute it before `BEGIN` and compare under the lock.
4. Exclude `finalized` from the periodic sweep unless the protocol changed.

**Red proof.**

- A DB test makes `preview_screening_identity` fail (delete the default AI route, or remove the sample report's `project_reports` row), runs `sweep_ai_first`, and asserts that the finalized cohort, its exclusions and the project's AI-first authority are unchanged.
- A second test changes the prompt hash and asserts the ADR §2.4 outcome.

**Done when:**

- Both tests pass.
- The existing `ai_first` DB tests still pass under packet 1's isolation.

The review's owner questions ask whether reopening finalized cohorts on identity drift was ever intended. Follow the ADR, and mention the behavior change in the commit body.

### Packet 4: `fix(assistant): plan-time revisions, record list, autonomy gate, provenance column` (P1)

**Problem.** Assistant `screen_reports` plans can apply up to 25 actions × 200 reports as the confirming human (`crates/ai/src/runtime/plan/policy.rs`: `MAX_ACTIONS_PER_PLAN=25`, `MAX_REPORTS_PER_ACTION=200`).

- **Stale writes slip through:** `run_screen_reports` (`crates/http-api/src/routes/assistant/plans.rs:631-728`) reads `expected_revision` at _confirm_ time (≈679-698, `.ok().flatten().unwrap_or(0)`), so the compare-and-set can never fire.
- **No AI governance:** it sets `exclusion_reason_id: None`. It bypasses `ensure_ai_task_enabled` (`review_preparation.rs:28-49`), calibration, blinding and exposure provenance.
- **Partial application:** each report gets its own transaction, so a mid-plan error leaves the plan half-applied.
- **The card hides the effect:** `AssistantPlanCard.svelte:94-103` shows only the model-written summary, rationale and a count.

Evidence: ai F2, sec S7.

**Fix**, in this order (one commit each is fine):

1. **Migration:** add `origin` to screening events (`'assistant_plan'`, null or `'direct'` otherwise), and plan-time revision storage in the plan's action args.
2. **API (plan creation):**
   - capture each report's current revision;
   - at confirmation, use it and reject stale items individually;
   - gate on the project's AI autonomy setting for the stage;
   - record `origin='assistant_plan'`.
   - Make PRISMA and ADR 0005's human reference stratum exclude `origin='assistant_plan'`.
3. **UI:**
   - the plan card lists the actual records (title, current status, target decision) taken from the action args, with per-item deselect;
   - derive the headline count and decision from the args, not from `summary`.
4. **Also (ai F3):** wrap assistant read-tool outputs that carry report or document text (`tools.rs:238`) in the same `<evidence-json>` envelope that structured tasks use. Add a system-prompt line (`catalog.rs:156-176`) saying tool results are untrusted data.

**Red proof.**

- An HTTP test creates a plan, changes one report's screening as another actor, confirms, and asserts that item is rejected as stale while the others apply.
- A test asserts an autonomy-off project refuses the plan.
- A SQL assertion checks that PRISMA's human count excludes `origin='assistant_plan'`.

**Done when:**

- The three tests pass.
- The card shows records and supports deselect.
- The codegen diff is committed.

The alternative "plans create review-run proposals instead of decisions" (ai F2 option 5) is a product change. Ask before taking it.

### Packet 5: `fix(server): safe deploy defaults` (P1; P0 if network-exposed)

**Problem.** The API is unauthenticated, but nothing keeps it local:

- **Binds every interface by default:** `crates/http-api/src/config.rs:17-18` defaults `API_BIND_ADDR` to `0.0.0.0:8080`.
- **Starts in local mode:** `crates/config/src/lib.rs:41` makes `APP_ENV` default to `local`, which disables hosted fail-fast guards. `apps/server/src/main.rs` loads dotenv files in every environment.
- **Open to DNS rebinding:** there is no `Host` validation.
- **Self-asserted actor kind:** clients can send `x-actor-kind: automation|system` (`crates/http-api/src/routes/actor.rs:22-23`) and forge AI provenance.

Evidence: sec S1, S2, S8; ops F10.

**Fix.**

1. Default the bind to `127.0.0.1:8080` in code. Containers set `0.0.0.0` explicitly, since `docker/rust-service.Dockerfile:150-151` runs `serve`.
2. Refuse to start on a non-loopback bind unless the operator explicitly opts in. Use an env var routed through `deepref-config`, e.g. `DEEPREF_ALLOW_NETWORK_BIND=1`, documented as "only behind an authenticating proxy". Set it in the Dockerfile or compose with that comment.
   - The trusted-identity design itself is a decision gate. This guard is the gate-free part.
3. Add a tower layer rejecting requests whose `Host` is not on an allow-list: `localhost`, `127.0.0.1` and `[::1]` with the configured port, plus the hosts of configured `API_CORS_ORIGINS`. Return 421 or 403.
4. Make `APP_ENV` default to `production` in the binary.
   - `.env.example` and `process-compose.yaml` already set `local`; keep dev working.
   - Load dotenv only when `APP_ENV=local`.
5. In `extract_actor`, accept only `x-actor-kind: user` from HTTP. Server code builds automation and system actors itself.
   - Make the deduplication body default (`routes/deduplication.rs:335-337`, `"web-user"`) use the same header actor as other routes (default `local-user`).
6. Add a README "Security model" note: single user, trusted machine, never expose the API without an authenticating proxy.

**Red proof.** These config and HTTP tests must fail before the fix:

- with no `APP_ENV` and no `DATABASE_URL`, config fails with "missing";
- a non-loopback bind without the opt-in refuses to start;
- a request with `Host: evil.example` is rejected;
- `x-actor-kind: automation` from a client is rejected.

**Done when:**

- The four tests pass.
- `just dev` still works locally.
- The container image still serves behind Caddy.

### Packet 6: `fix(jobs): attempt-aware recovery and panic isolation` (P2, Stage-0 part of the queue work)

**Problem.**

- **Recovery ignores attempts:** `recover_expired_jobs` (`crates/app/src/jobs.rs:59-66`) requeues expired jobs without checking `max_attempts` and without backoff. `CLAIM` (≈12-28) has no attempts predicate. A job that crashes the worker (poison PDF, OOM, PDFium segfault) is reclaimed forever and never reaches `dead`.
- **One panic kills the worker:** in `services/worker/src/lib.rs:124-133`, a job task panic (`JoinError`) ends the whole worker loop.

Evidence: data F4, ops F2.

**Fix.**

1. Recovery: `state = CASE WHEN attempts >= max_attempts THEN 'dead' ELSE 'queued' END`, `last_error = 'lease expired'`, `available_at = now() + backoff(attempts)`.
2. Add `attempts < max_attempts` to `CLAIM` as a second guard.
3. In the worker, on `JoinError::is_panic`, call `fail_job` with the panic message and keep the loop alive.
4. `warn!` on every job failure and `error!` on dead-lettering, with `job_id`, `kind` and `attempts`. Today the `Err` branch at `lib.rs:255-270` logs nothing (ops F3).

**Red proof.**

- A DB test claims a job with `max_attempts=1`, expires the lease, runs recovery, and asserts `dead`.
  - The existing `expired_leases_are_fenced_and_retries_reach_dead_at_max_attempts` covers the graceful `fail_job` path. Read it first, and extend it or add a sibling test.
- A worker test with a panicking handler asserts the job is failed and the loop continues.

**Done when:** both tests pass, and the job-failure log line appears in test output.

### Packet 7: `fix(jobs): claim-token fencing, workflow step fencing, renewal retry` (P2)

**Problem.**

- **The lease token is per process, not per claim:** the owner string is created once per process (`services/worker/src/lib.rs:92`). `renew_job`, `complete_job` and `fail_job` (`jobs.rs:119-183`) match only the owner. `claim_job` recovers expired jobs before claiming (`jobs.rs:76`), so the same process can reclaim its own expired job while the stale attempt can still complete it.
- **Workflow steps have no fence:** `begin_node_execution` / `finish_node_execution` (`crates/app/src/workflows.rs:1338-1460, 1519-1620`) don't fence at all. Late duplicates overwrite results, and HTTP, email and screening side effects can run twice.
- **A renewal error cancels work:** one renewal error (`lib.rs:294`, `renew_job(...).await?`) drops the in-flight future mid-execution.

Evidence: data F3.

**Fix.**

1. `claim_job` returns a `JobLease { id, owner, attempt }`. Renew, complete and fail require `AND attempts = $attempt`. Thread `JobLease` through, instead of `&str owner`.
2. Add `claimed_attempt` on `workflow_node_runs`. Begin and finish compare-and-set on it, and finish is a no-op unless `status = 'running'`.
3. Retry renewal with backoff before giving up the lease.

Automation steps already fence correctly on `claimed_job_attempt` (`automations.rs:526-597`); copy that pattern.

**Red proof.**

- With a lease of a few ms: claim, expire, reclaim with the same owner, then call `complete_job` with the first attempt and expect `false`.
- Two concurrent `handle_step` calls on one workflow node produce one execution and one outcome.

**Done when:** both tests pass.

### Packet 8: `fix(review): allow retry and regenerate of terminal review runs` (P2)

**Problem.** A failed or blocked review run can never be retried. The key `review:{manifest_hash}` (`crates/app/src/review_runs.rs:182`) contains no actor, time or generation. `dispatch_automation_trigger` (`crates/app/migrations/0019_automation_agent.sql:429-440`) returns the existing run whatever its status. After a provider outage, a record's "AI suggestion" stays `failed` until something changes the manifest.

Evidence: ai F1.

**Fix.**

1. Separate double-submit dedupe from run identity. Add a generation number: if the existing run is `failed` or `blocked`, or `completed` with a rejected proposal and `regenerate: true`, create generation n+1 with key `review:{manifest_hash}:{n}`.
2. Plain double-submits still return the in-flight or completed run.
3. Add "Retry" and "Regenerate" to the review-run observer UI (`apps/web/src/lib/features/ai-assistance/review-run-observer.svelte.ts` and its consumers).
4. Apply the same fix to `second_review_attempts`, which is claimed once per report and protocol.

**Red proof.** A DB test schedules a run, calls `fail_review_run`, schedules again, and asserts a new run id in `queued`. Repeat for `blocked`.

**Done when:** the tests pass, the UI exposes Retry, and codegen is committed.

### Packet 9: `feat(api): persistence error classification and real correlation ids` (P2)

**Problem.**

- **Transient DB errors become a 400 with raw SQL text:** in `crates/app/src/screening.rs:204-206,276`, `.map_err(|e| ScreeningError::InvalidTransition(e.to_string()))` wraps DB errors, and `routes/review.rs:664` turns them into `400 Bad Request` with raw sqlx text.
- **No error classification:** `error.rs:62-82` maps every non-`RowNotFound` sqlx error to 500. There are five hand-rolled SQLSTATE checks (`extraction.rs:915`, `workflows.rs:216`, `assistant*.rs`, `automations.rs:609`, `documents.rs:1088`), and no handling of 40001/40P01.
- **The correlation id matches nothing:** `error.rs:157` sets `correlation_id: Some(Uuid::new_v4())`, a random value, while the middleware's id (`routes/mod.rs:305-319`) is unused.

Evidence: data F11, ops F3.

**Fix.**

1. In `deepref-app`, add `classify(&sqlx::Error) -> PersistenceError { NotFound, Conflict(constraint), Retryable, Integrity, Other }`, plus a `with_retry` helper for serializable or repeatable-read transactions.
2. Map to `ApiError`: Retryable → 503 with Retry-After, Conflict → 409, Integrity → 409 or 422, Other → 500 with no raw text.
3. Replace the five hand-rolled checks.
4. Give screening errors a `Database` variant instead of stringifying them into `InvalidTransition`.
5. Put the request's correlation id into `ApiError` bodies and into an INFO-level `TraceLayer` span: correlation id, route template, status, latency.

**Red proof.**

- `classify` unit tests per SQLSTATE (23505, 23503, 40001, 40P01).
- An HTTP test: the error body's `correlation_id` equals the request's `x-correlation-id`.
- A screening DB error returns 5xx, not 400.

**Done when:** these tests pass and no `InvalidTransition(e.to_string())` remains.

### Packet 10: `feat(ops): truthful readiness, job spans, worker heartbeat` (P2)

**Problem.**

- **Readiness checks the wrong schema version:** `crates/http-api/src/routes/health.rs:95-114` requires schema version ≥ 10, but the code needs 58.
- **The worker has no health surface.**
- **Job failures can't be traced:** they carry no span.
- **The build SHA is never read:** `DEEPREF_BUILD_SHA` is set in the image and read nowhere.

Evidence: ops F3, F9.

**Fix.**

1. Derive the required version from `deepref_app::MIGRATOR.iter().map(|m| m.version).max()`. Fail readiness when the DB is behind, and warn when it is ahead.
2. Open a `job` span in `process_claimed_job` with `job_id`, `kind`, `project_id`, `attempt`, and the correlation id. Persist the correlation id on enqueue.
3. Add a worker heartbeat row (`worker_heartbeats(owner, seen_at)`) and report it in `/health/dependencies`.
4. Set `service.version` from `DEEPREF_BUILD_SHA`.
5. RED metrics and queue gauges need a new dependency (axum-prometheus vs an OTel meter). Propose the choice to the user before adding one.

**Red proof.**

- A test asserts the readiness requirement equals the max embedded migration.
- A forced job failure emits a log line containing `job_id`.

**Done when:** both tests pass.

### Packet 11: `ci: cover packages/ui, fix scheduled lanes, harden publish-images` (P2/P3)

Evidence: ops F4, F6, F8.

1. Add `packages/**` to the `web` and `containers` path filters (`ci.yml:48-68`), and run `pnpm --filter @deepref/ui test` in the Web job.
2. Fix the scheduled lanes, each a one-line fix:
   - **fuzz:** add `exclude = ["fuzz"]` to the root `Cargo.toml` workspace.
   - **benchmarks:** `cargo bench --workspace --benches -- --save-baseline main` (`benchmarks.yml:58`).
   - **ts-mutants:** use `node-version: 24.21.0` instead of `node-version-file: .mise.toml`.
3. Add failure notification for scheduled workflows.
4. In `publish-images.yml`:
   - pass `inputs.tag` through `env:` instead of interpolating it into `run:` (≈`:41`);
   - stop pushing `:latest` on pre-release tags;
   - gate on a green CI run for the SHA.
   - Native per-arch builds are optional here; leave them as a follow-up.
5. Delete the `ai-eval-security-gates` job, the PR-time jscpd run, and the unused ripgrep install (§R, CI row).

**Done when:**

- `actionlint` passes.
- A `workflow_dispatch` run of fuzz, benchmarks and ts-mutants succeeds (ask the user to trigger it if you lack permission).
- A no-op `packages/ui` change triggers the Web steps.

### Packet 12: `fix: frontend data correctness and smaller backend defects` (P2; split into one commit per item)

Each item is independent. For each one, write the red test from its evidence section first.

| Item                                                                                                             | Location                                                                                                                                                                             | Fix                                                                                                                                                                                 | Evidence |
| ---------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------- |
| Document blocks capped at 100, so overlays vanish on long papers                                                 | `crates/http-api/src/routes/documents.rs:22` (`MAX_DOCUMENT_LIST_LIMIT=100`, no cursor). UI: `FullTextScreening.svelte:176-185`, `AppraisalScreen.svelte`, `ExtractionScreen.svelte` | cursor or per-page block fetch; at minimum, a "showing first N" notice when the result is truncated                                                                                 | fe F2    |
| AI-proposal invalidation misses the TA queue, causing self-inflicted 409s; it also refetches every project query | `apps/web/orval.config.ts` invalidation targets; `ScreeningFocus.svelte:81-104` hand key `['screening-queue',…]`                                                                     | use generated query keys; point invalidations at `getScreeningQueue` with `params: ['projectId']`; add a test that no generated invalidation emits a bare `'/api/projects/'` prefix | fe F3    |
| A transient `getProject` error navigates to a different project                                                  | `ProjectWorkspace.svelte:38-43,107-113`, `workspace-navigation.ts:9-20`                                                                                                              | fall back only on a 404 from the initial load; otherwise show an inline alert with retry; show an explicit "Project not found" state                                                | fe F4    |
| Assistant runs orphaned on `Terminate`, so the UI polls forever                                                  | worker `assistant.rs:52-56,117-125,194-196,243,368`; `run_events.rs:78` `.unwrap_or_default()`                                                                                       | every `Terminate` calls `fail_run` first; add a reconciler pass failing runs whose job is dead or missing; surface DB errors instead of "no events"                                 | ai F4    |
| Citation depth unbounded                                                                                         | `acquisitions.rs:183-206`, `settings.rs:82`                                                                                                                                          | domain `MaxDepth` (≤ 3) plus DB CHECKs; depth 10 → 400                                                                                                                              | data F9  |
| Workflow HTTP secrets follow an edited URL                                                                       | `services/worker/src/workflows/nodes.rs:340-372`, `crates/app/src/workflows.rs:276-310,383-394`, `validate.rs:220-222`, `conditions.rs:191-211`                                      | bind each secret to the origin it was entered for and drop it when the origin changes; require a fixed origin (no `{{` before the path); percent-encode substituted values          | sec S3   |
| Hook rate limiter unbounded; unsigned webhooks accepted                                                          | `routes/workflows.rs:1473-1492,1612-1621`                                                                                                                                            | bounded LRU keyed after token lookup; `signature_required=true` for new webhooks                                                                                                    | sec S9   |
| Ingestion cancel accepts any UUID and completed runs                                                             | ingestion cancel route                                                                                                                                                               | status guard plus existence check; 404 or 409                                                                                                                                       | data F5  |
| Assistant hardcodes `x-actor-id: local-user`                                                                     | web assistant data layer                                                                                                                                                             | use `currentReviewerId()` and the generated client                                                                                                                                  | fe F9    |

**Done when:** each item has a test that went red and is now green, and all verification gates pass.

---

## Decision gates (ask the owner first)

These recommendations change product scope, delete features, or pick a long-lived design. Present the review's recommendation and ask; do not implement them unprompted.

| Decision                                   | Review recommendation                                                                                                                                | Where         |
| ------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------- | ------------- |
| Identity model                             | Trusted forward-auth header (`X-Forwarded-User` from oauth2-proxy or Authelia) plus a `project_members` roster now; built-in sessions only if hosted | §Q4, prod H1  |
| Dual human screening                       | Generalise `ai_reviewer_decisions` into `reviewer_opinions`, with `screening_state` as the consensus projection; depends on identity                 | §O H2         |
| Workflow engine scope                      | Park the Slack/HTTP/webhook/inbound-email nodes; expose 4–5 purpose-built automations                                                                | §O P1, S4     |
| Calibration bundles                        | Park: no production producer or consumer                                                                                                             | §O P2, ai F9  |
| Legacy ingestion plane                     | Contract it over three releases (expand/contract)                                                                                                    | §R, data §5.2 |
| AI execution stack                         | Flatten onto an `ai_calls` ledger; definitions as Rust fns                                                                                           | §Q3, ai §5    |
| `candidate_audit` + `semantic_repair` loop | Remove after an offline evaluation on a labelled set                                                                                                 | ai F6         |
| Use-case layer                             | Collapse crates first (Option C), then pilot `app::screening` (Option B)                                                                             | §Q1, arch §5  |
| Assistant scope                            | Read-only "ask your corpus"; writes go through proposal queues                                                                                       | §O P5         |
| Extraction default (`Act`)                 | Default to `suggest`, or block export while values are unverified                                                                                    | §O P9         |

## After the packets

Pick these up only once packets 1–12 are done, or when the user points you to them:

- **Deletion plan (§R):** rows marked confidence High with no prerequisite are mechanical. Do them as one `chore:` change per row, separate from fixes.
- **Stage 2–3 roadmap (§S):**
  - `JobKind` enum;
  - all config through `deepref-config` (32 stray `std::env::var` reads, arch A4);
  - coalesced metric recompute with one batched UPDATE (data F7);
  - `FOR NO KEY UPDATE` instead of `FOR UPDATE` on the project row (data F10);
  - revision CAS on extraction, appraisal and studies (data F13);
  - metering every LLM attempt (ai F5).
- **Docs and agent guidance (ops F14):** fix the broken `AGENTS.md` links and the nonexistent `packages/core`, and add the Rust gates to its verification section. That is an edit to always-loaded agent context: show the user the diff before committing.

## Reporting back

When you finish (or stop), report to the user:

- each packet's status: `fixed`, `partial` (and what is left), `no_change_needed` (and why), or `blocked` (on which decision);
- the red-then-green test for each fix;
- the final verification-gate results, with counts compared to the baseline above;
- anything you found that contradicts this handoff.

## Integration results (2026-10-14)

The recovered Claude work is integrated on `integration/review-resume` in `.worktrees/review-resume`. The original dirty checkout was preserved separately; its interrupted state is captured by `3983b9b`. The owner decisions in the [agent brief](2026-10-11-agent-brief.md#owner-decisions-2026-10-11-for-wave-2) supersede the original publishing and ingestion-retirement gates above.

### Packet disposition

| Packet | Status                                     | Integrated behavior and verification                                                                                                                                                                                                                                                                                                                                 |
| ------ | ------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1      | fixed locally; remote CI evidence separate | Per-test migrated databases, required-DB mode, digest-keyed templates, serialized reaping and bounded connection budget. Full parallel PostgreSQL run: 957 passed, 3 skipped; no failures.                                                                                                                                                                           |
| 2      | fixed                                      | Repeat and re-entrancy guards cover title/abstract and full-text screening. In the live full-text UI, held I produced one decision for the displayed report and none for its successor.                                                                                                                                                                              |
| 3      | fixed                                      | Unknown identity preserves scientific state; finalized-cohort semantic drift remains non-destructive. `identity_database_failure_rolls_back_without_aborting_the_sweep` and the AI-first suites pass. The separate ADR0005 reopening question below is unchanged.                                                                                                    |
| 4      | fixed                                      | Host-owned planning snapshots, individual stale rejection, deselection, autonomy checks and assistant provenance. Live confirmation returned 200 and persisted `deselected=1, stale=1, applied=0`; the UI displayed those outcomes. Assistant PostgreSQL suites pass.                                                                                                |
| 5      | fixed                                      | Loopback binding, explicit network opt-in, exact hosted CORS origins, host guard and rejection of forged system actors. Live hostile Host returned 403 and forged actor returned 400. Release image refuses missing hosted configuration.                                                                                                                            |
| 6      | fixed                                      | Attempt-aware recovery and dead-lettering; handler panics isolated from unrelated jobs. Queue and worker claim-loop suites pass. Obsolete formatted-log assertions were removed; durable failure/retry outcomes remain asserted.                                                                                                                                     |
| 7      | fixed                                      | Owner/attempt/live-lease fencing, guarded durable step transitions and bounded renewal waits. Queue, automation, workflow and populated-cutover suites pass. Live cancellation during a queued wait prevented the successor notification.                                                                                                                            |
| 8      | fixed                                      | Retry and fresh review generations, regeneration UI, bounded second-review retry and transactional autonomy outbox. `exhausted_expired_review_claim_settles_history_and_opens_a_new_generation` passes; worker outbox and review-run suites pass.                                                                                                                    |
| 9      | fixed                                      | Typed persistence classifications, bounded transactional retry, correlation propagation and truthful HTTP status handling. Full API suites pass; live response and request span correlation IDs match.                                                                                                                                                               |
| 10     | fixed within approved scope                | Embedded migration readiness, worker heartbeat, job spans and build-version configuration. Actual release image reports schema 81 / required 81. Additional RED-metrics dependency remains owner-gated.                                                                                                                                                              |
| 11     | fixed                                      | UI path coverage, repaired fuzz/benchmark/mutation lanes, scheduled failure reporting and exact-SHA image publication gate. Actionlint passes; all three hosted dispatches succeed. Stable/prerelease bake tag selection checked; actual release API image built and served readiness.                                                                               |
| 12     | fixed                                      | Cursor-paged evidence, project-scoped invalidation, selected-project error recovery, settled assistant runs, depth bounds, origin-bound HTTP secrets, bounded authenticated hooks, guarded acquisition cancellation and reviewer-aware assistant calls. PostgreSQL and web suites pass; live evidence paging/retry, cancellation and hook gate scenarios below pass. |

### Approved architecture work

| Track | Status | Cutover                                                                                                                                                                                                                                                                        |
| ----- | ------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| H1    | fixed  | Collapsed pure domain/integration crates and removed dead prototypes/exports. Workspace boundary check validates 13 members across dependency kinds and targets.                                                                                                               |
| H2    | fixed  | Typed job/lifecycle row seams and centralized runtime configuration; consumers migrated.                                                                                                                                                                                       |
| H3    | fixed  | `deepref_app::screening` owns the screening use-case boundary; HTTP/assistant consumers migrated.                                                                                                                                                                              |
| H4    | fixed  | Workflow, purpose-built automation and review lifecycle persistence share `durable_runs`, `durable_steps`, `durable_attempts` and `durable_run_step` dispatch. Niche Slack/HTTP/webhook/inbound-email nodes default off; outgoing email and existing graph readability remain. |
| H5    | fixed  | Expanded/backfilled canonical acquisitions, switched DOI API/worker/web consumers and dropped the old ingestion persistence plane in this round. Populated acquisition and durable-run migration regressions pass.                                                             |

### Final local gates

| Gate                                                                         | Observed result                                                            | Review baseline                            |
| ---------------------------------------------------------------------------- | -------------------------------------------------------------------------- | ------------------------------------------ |
| `cargo fmt --all -- --check`                                                 | pass                                                                       | pass                                       |
| `cargo clippy --workspace --all-targets --locked -- -D warnings`             | pass                                                                       | pass                                       |
| `cargo nextest run --workspace --lib --bins --locked`                        | 620 passed, 2 skipped                                                      | 569 passed, 2 ignored                      |
| `cargo nextest run --workspace --tests --locked --profile ci --no-fail-fast` | 957 passed, 3 skipped, 0 failures                                          | 837/856 parallel; 19 interference failures |
| `cargo test --doc --workspace --locked`                                      | pass; 1 ignored example                                                    | not recorded                               |
| `cargo xtask boundaries`                                                     | pass; 13 workspace members                                                 | pass                                       |
| `cargo xtask sqlx prepare` / `cargo xtask sqlx check`                        | preparation and offline metadata parity pass                               | pass                                       |
| `mise exec -- pnpm generate:api:check`                                       | pass after SQLx metadata preparation                                       | pass                                       |
| `mise exec -- pnpm check`                                                    | web/UI: 0 errors, 0 warnings                                               | 0 errors                                   |
| `mise exec -- pnpm test`                                                     | web 536 passed; UI 26 passed                                               | web 535; UI 26                             |
| `mise exec -- pnpm lint`                                                     | 0 errors, 63 existing no-restyle warnings                                  | 0 errors, 63 warnings                      |
| `actionlint`                                                                 | pass                                                                       | not recorded                               |
| Docker bake API release build                                                | built `deepref-review-api:local`; actual container served ready, schema 81 | not recorded                               |

Rust checks used the integration worktree's target directory, two build jobs, no incremental/debug output and SQLx offline mode except preparation. PostgreSQL tests used disposable port 55459 with `DEEPREF_REQUIRE_DATABASE=1`; the development database on 5432 was not used. API generation initially failed on obsolete SQLx metadata; regeneration followed preparation and the parity gate then passed.

### Actual runtime proof and limits

- Cached DOI acquisition and refresh traversed the actual API/worker: create 201, idempotent replay 200, completed graph revision 4 with two nodes/one edge and watermark lag 0. Unknown cancel/refresh returned 404, completed cancel 409 and queued cancel 202.
- Manual workflow completed two durable steps; replay returned the same run. A separate wait/cancel run ended cancelled with no successor notification. Purpose-built maintenance completed with one persisted durable step and attempt.
- Flag-off catalog marked four niche nodes unavailable while retaining outgoing email. Flag-on creation preserved a signed-hook graph; flag-off API could read it. Unsigned delivery was rejected 403; signed delivery was accepted 202 but the flag-off worker failed the run without notification side effects.
- Real full-text UI found a citation beyond the first 100 queue reports and loaded all 201 evidence blocks across cursor pages. Page-four PDF/cited passage were visible. Injected later-page 503s showed a retryable evidence error; Retry restored complete evidence and preserved the citation URL.
- Changing reports dismissed the old exclusion picker without writing a decision. Held-key screening wrote once, not onto the next report.
- Real assistant client consumed persisted sequence 1, then a temporary database relation/connection outage produced `assistant_events_unavailable` without advancing the cursor. Resume from 1 received only sequences 2–3 and one recovered token/done pair.
- Browser plan fixture confirmed deselection and stale-snapshot rejection through the real confirmation API and database. Plan generation and document parsing were seeded fixtures, not real model/parser executions. No external provider, mail, Slack or HTTP destination was exercised. Storage smoke was local; production S3/HTTPS configuration is not validated.

The recovered branches contain regression coverage, but agents deferred execution to integration. This report does **not** assert every historical test was observed red before its original fix. Observed integration failures were repaired and rerun green, followed by the full suites. Deliberate source inversion in hosted CI and model-backed/parser-backed acceptance runs are not claimed.

### Contradictions and remaining owner gates

- The handoff's suggested `--benches -- --save-baseline` also selects libtest harnesses, which reject the Criterion argument; the repaired lane selects explicit benchmark targets with `--bench '*'`.
- Historical citation defaults above three are preserved for upgrade safety; new writes are bounded. Operators must explicitly choose a valid replacement before changing those historical settings.
- Shipped migration checksums are preserved. Test fixtures and old-field compatibility assertions tied solely to the retired ingestion plane were migrated or removed rather than perpetuating aliases.
- Live waiting workflow steps are exposed as queued nodes; cancellation returns 204. Hook authorization denial is 403, not an assumed 401.
- ADR0005 §2.4 versus existing `human_state_changed` / `automation_miss_discovered` reopening of other finalized exclusions remains a separate owner decision; no broadened sweep was introduced.
- Identity/roster redesign, dual human screening, metrics dependency, calibration deletion, AI-ledger flattening and assistant read-only conversion remain unapproved and unchanged.

### Hosted verification and post-publication repairs

- [Benchmarks, run 38591248268](https://github.com/luzes-dev/deepref/actions/runs/38591248268): success.
- Fuzz first failed because `rustup default nightly` did not override the repository's pinned stable toolchain. `7db5e7f` sets a directory override and an explicit glibc target; [run 38598652144](https://github.com/luzes-dev/deepref/actions/runs/38598652144) passed all three fuzz targets.
- The unsharded TypeScript mutation run timed out after 45 minutes. Its 584 static mutants required full-suite reloads on one runner. `6c675c1` partitions the original mutation globs into 16 source-balanced shards with two runners per shard. A local invariant smoke proved all 72 original source files appear exactly once, with static-mutant behavior and thresholds unchanged. [Run 38599370312](https://github.com/luzes-dev/deepref/actions/runs/38599370312) passed all 16 shards and uploaded separate reports.
- [Initial integrated CI, run 38591195167](https://github.com/luzes-dev/deepref/actions/runs/38591195167), exposed four outdated browser contracts: retired acquisition selectors and bare-array block mocks in appraisal/full-text. Fixtures now use generated acquisition and cursor-page types; appraisal evidence spans two pages. The repaired local browser E2E run passed 78/78 after installing the missing Playwright browser. The retired prototype links in the backend/UI handoff were repaired, and `scripts/check-docs.sh` passes.
- Base PR #209's [Security run 38590735320](https://github.com/luzes-dev/deepref/actions/runs/38590735320) rejected locked `rustls 0.23.45` for four advisories, RUSTSEC-2026-0346 through RUSTSEC-2026-0349. Integration commit `764a8a2` patches to `0.23.46`, without advisory suppression. `cargo deny check` passes; patched workspace clippy and 620 unit tests pass, and the patched server executes `--print-openapi`. The earlier Docker runtime smoke preceded this patch. The base PR still carries the older lockfile; apply the reviewed patch before treating its Security check as green.
- After browser-fixture repairs, `pnpm check` again reports zero errors/warnings, web/UI tests pass 536/26, lint retains the 63-warning baseline, and the existing visual lane passes 110 tests with 2 skips across its four viewport/theme projects. No screenshot baselines were regenerated.
