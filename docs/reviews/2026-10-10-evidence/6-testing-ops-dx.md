> Historical review. This document describes the pre-SaaS architecture and is not normative documentation.

# Reviewer 6: Testing, CI/CD, Build/Deploy, Observability, DX, Docs, Config

Repo: /home/luzes/Code/deepref @ b0ad219 (main, clean). Review date 2026-10-10.
I did not modify the repo. Evidence comes from reading files, `gh` (run logs, artifacts, ruleset), and `bash scripts/check-docs.sh` (it passed). I tried to run the DB-backed integration suite locally against a throwaway pgvector container, but the disk filled up, which was an environmental problem, not a repo defect. I stopped and removed the container.

---

## 1. Maps

### 1.1 Test pyramid (as built)

| Layer | Where | Size | Runs in CI? | Notes |
|---|---|---|---|---|
| Rust unit (`#[cfg(test)]`) | all crates | ~850 tests in total, ~4.8 s | yes (`Rust / test`) | Strong in ai, review, application, domain. |
| Rust "integration", pure | `crates/ai/tests` (eval_gate, prompt_injection, screening_normalization), `crates/documents/tests` (gated on `PDFIUM_LIBRARY_PATH`), `crates/config/tests` | ~2.4k LOC | yes | eval_gate scores a checked-in prediction fixture. No model is called. |
| Rust "integration", DB-backed | `crates/app/tests` (35 files, 17k LOC), `crates/http-api/tests` (22 files, 9.8k LOC), `services/worker/tests` (13 files, 3k LOC) | ~30k LOC | **no. They skip silently** (finding F1) | Each test starts with `let url = std::env::var("DATABASE_URL").ok()?;` and returns `Ok(())` when it is unset. |
| Source-text "tests" | `services/worker/tests/{crash_boundaries,persistence_boundaries,global_throttle,reconciliation}.rs`, `crates/http-api/tests/migration.rs`, `crates/app/tests/migration_001[3-5].rs` (partly) | small | yes | Built on `include_str!(...)` plus `contains()` (§4). |
| Web unit/component (vitest, node + jsdom projects) | `apps/web/src/**` | 73 spec files, ~8k LOC | yes (`Web`, with coverage) | MSW is used in only 2 specs, so there is no over-mocking. Leftover scaffold `src/lib/vitest-examples/greet.*`. |
| UI package vitest | `packages/ui` | 5 spec files | **never in CI** (F4) | `pnpm --filter @deepref/ui test` is not invoked anywhere in CI. |
| UI package Playwright visual | `packages/ui` `test:visual` | n/a | never | |
| Browser "E2E" (Playwright) | `apps/web/tests/settings-shallow.e2e.ts` and `tests/visual/*.spec.ts` × 4 viewport/theme projects | ~1.7k LOC | yes (`Browser E2E`) | **Fully mocked API**: `tests/visual/api-fixture.ts` intercepts `**/api/**` and refuses unmodelled requests. No real backend at any level. |
| Property tests | Rust `property_tests` modules, `@fast-check/vitest` | small | yes, through the normal suites | |
| Mutation (Rust, cargo-mutants) | weekly | domain/application/review/graph | report-only, `|| true` | Latest run (37195437668): **250 caught / 332 missed / 128 unviable**, a 43% kill rate (F7). |
| Mutation (TS, Stryker) | weekly | features/*.ts | **failed 4/4 runs, never produced a report** (F6) | |
| Fuzz (cargo-fuzz) | weekly | 3 targets | **failed 4/4 runs** (F6) | |
| Benchmarks (criterion) | weekly | workspace | **failed 4/4 runs** (F6) | |

### 1.2 CI pipeline (`.github/workflows`)

- **ci.yml** (PR and push to main), path-filtered by `dorny/paths-filter` into `rust`, `web` and `containers`:
  - `docs`: link check only.
  - `rust` matrix: fmt (plus `xtask boundaries` and xtask tests), clippy, and `test` (`nextest --lib --bins --tests --examples` plus doctests, **no DB**).
  - `ai-eval-security-gates`: re-runs two `deepref-ai` test binaries that `rust/test` already ran.
  - `coverage`: main only. llvm-cov feeds an artifact, which `rust-risk.yml` (cargo-crap) consumes.
  - `web`: lint, check, vitest coverage, build.
  - `fallow`: PR only, new-only gate.
  - `duplication`: jscpd, report-only.
  - `codegen`: OpenAPI and Orval drift through `scripts/check-api-codegen.sh` into a temp dir, then `git diff --exit-code`.
  - `full-stack`: compose Postgres, `sqlx migrate run`, `xtask sqlx check` against the live DB, then `nextest --tests` **with no DATABASE_URL** (F1).
  - `browser-e2e`: Playwright with mocks, no backend.
  - `containers`: amd64 build `output=cacheonly` on PR, plus arm64 natively on push.
  - Aggregators `Rust`, `E2E`, `Docker`.
- **Ruleset `main-protection`** requires: Rust, Web, Fallow, OpenAPI and Orval drift, E2E, Docker, Security (strict). `Documentation contracts`, `Duplication`, and `AI evaluation and security gates` are not required.
- **security.yml**: CodeQL (rust, js/ts), cargo-deny, cargo-shear, `pnpm audit --audit-level high`, an action-SHA-pin policy, actionlint, shellcheck. All on every PR.
- **Scheduled**: rust-mutants, ts-mutants, fuzz, benchmarks (all Sunday). rust-risk runs on `workflow_run`.
- **publish-images.yml**: `v*` tag or dispatch. Bake api, worker and web for amd64 and arm64 (QEMU), pushed to GHCR. **It has never run.**
- Caching: sccache through GHA (test job is RW, others RO), cargo registry cache, BuildKit `type=gha`, sccache injected into BuildKit through cache-dance. Last main run took 5–10 min wall clock. All actions are SHA-pinned with version comments, and that is policy-enforced.

### 1.3 Build and deploy topology

- One Rust binary, `deepref-server`, with roles `serve | worker | all | migrate | import-legacy | --print-openapi` (`apps/server/src/command.rs`). No args means `all`.
- `docker/rust-service.Dockerfile`:
  - Multi-stage: toolchain → cargo-chef planner → cook → build → PDFium download (checksum and VERSION verified) → `debian:bookworm-slim` (digest-pinned) runtime.
  - Two final targets, `api` (`CMD serve`) and `worker` (`CMD worker`). Each image carries the whole binary.
  - UID 65532, STOPSIGNAL SIGTERM, CA bundle copied from the builder, `PDFIUM_LIBRARY_PATH=/usr/local/lib/libpdfium.so`.
- `apps/web/Dockerfile`: node builder (BUILDPLATFORM) builds the SPA, then a Caddy runtime as UID 65532. The `Caddyfile` serves `/healthz` and reverse-proxies `/api/*` to `{$DEEPREF_API_UPSTREAM:deepref-api:8080}`, falling back to the SPA.
- **No deploy manifests in the repo.** `charts/deepref` (Helm, ADOT collector) and `infra/` were deleted in e70e800 ("remove deployment slop"). `docs/api.md:20` still says "Hosted migrations run as the Helm/Argo PreSync Job".
- Migrations are an explicit `deepref-server migrate` step (ADR 0001). `serve` does not migrate. It does write to the DB at startup (`ensure_default_model_routes`).
- Health:
  - API: `/health/live` (uptime), `/health/ready` (schema ≥ **10**, hard-coded, F9), `/health/dependencies` (PG ping plus job backlog/dead/stale), and the deprecated `/health`.
  - Worker: **no health surface**.
  - Web: `/healthz`.
- Shutdown:
  - Worker: drains active tasks and the reconciler, each bounded by `SHUTDOWN_DEADLINE_SECS` (default 30).
  - API: `axum::serve(..).with_graceful_shutdown(..)` with **no deadline** (F12).
  - OTel: flushed with the deadline.

### 1.4 Telemetry path

- `crates/telemetry` (115 LOC): `tracing_subscriber` JSON fmt layer, always JSON, also in local dev. When `OTEL_EXPORTER_OTLP_ENDPOINT` is set it adds an OTLP gRPC span exporter.
  - No text-map propagator is installed, so `traceparent` is not extracted or injected.
  - No metrics.
  - `current_trace_id()` exists and is unused.
- HTTP:
  - `TraceLayer::new_for_http()` defaults: span and response events at DEBUG, failures (5xx) at ERROR. With the default `RUST_LOG=info` there is **no access log and no request span fields**.
  - The `correlation` middleware accepts or creates `x-correlation-id` and echoes it, but never records it on a span or log.
  - `ApiError::into_response` puts a **fresh random `Uuid::new_v4()`** into the body's `correlation_id` (`crates/http-api/src/error.rs:157`).
  - The web client prefers the body value (`apps/web/src/lib/api/custom-fetch.ts:36-38`). F3.
- Jobs: no trace or correlation context is stored with a job. The worker logs at a few sites (~40 log statements across the worker), and **a failing job's error is not logged**, only written to `jobs.last_error` (`services/worker/src/lib.rs:255-270`). F3.
- LLM calls: `crates/ai/src/runner.rs` has two `info_span!`s. Usage is persisted to a ledger table.
- Alerts, dashboards, SLOs: none in the repo. `observability/` contains only `synthetics/node_modules` (untracked leftover).
- Runbooks: `docs/assistant-durability-runbook.md` and `docs/deployment-recalibration.md`. Both are detailed but written as code walkthroughs with 27 `file.rs:line` references each. They are not organised as symptom → query → action.

---

## 2. What is good

- **Supply-chain hygiene is unusually strong.**
  - Every action is SHA-pinned, and a CI policy enforces it.
  - Every downloaded binary is checksum-verified (cargo-mutants, cargo-fuzz, cargo-crap, cargo-chef, sccache, PDFium).
  - Base images are digest-pinned.
  - Images are non-root and carry SBOM and provenance attestations.
  - cargo-deny and pnpm audit run, `persist-credentials: false` is set everywhere, and `permissions: contents: read` is the default.
- **The codegen drift guard is well built.** `scripts/check-api-codegen.sh` regenerates into a `mktemp` tree and diffs, and CI then proves `git diff --exit-code`. It is a required check.
- **The SQLx offline cache is verified against a live, migrated DB in CI** (`cargo xtask sqlx check`), so `.sqlx` drift is caught.
- **Aggregator jobs** (`Rust`, `E2E`, `Docker`, `Security`) keep required checks stable under path filtering.
- **The DB test code itself is good where it exists.**
  - `crates/app/tests/jobs.rs::expired_leases_are_fenced_and_retries_reach_dead_at_max_attempts` checks stale-owner fencing for renew, complete and fail without wall-clock sleeps.
  - `calibration_stage_migration.rs` applies migrations up to N on a fresh database, writes legacy rows, then applies the rest. That is a real data-migration test.
  - The problem is that CI never executes them (F1).
- **The visual and e2e fixtures are typed against the generated OpenAPI models** and refuse unmodelled requests. That keeps them honest about request shape, though not about backend behaviour.
- **Config is fail-fast for hosted environments.**
  - A non-local `APP_ENV` requires `DATABASE_URL`.
  - `API_CORS_ALLOW_ANY` is local-only, and hosted CORS needs at least one origin.
  - `DOCUMENT_STORAGE_BACKEND=local` is local-only.
  - Pool bounds and zero-durations are validated.
  - `DATABASE_URL_FILE` secret-file support exists.
- **Worker graceful drain has a deadline**, and lease renewal runs at lease/3.
- **ADR 0001** makes migration an explicit, auditable step.
- **The coverage → cargo-crap risk pipeline** reuses the coverage artifact and verifies its revision instead of re-running tests.

---

## 3. Ranked findings

### F1. "Full-stack integration" never runs a single DB-backed test, and the DB integration suite has apparently never run in CI
- **Category:** Defect (CI gap). **Severity:** P1, the highest-impact item in this lens. **Confidence:** High.
- **Evidence:**
  - In `.github/workflows/ci.yml:579-581`, the step `Run integration tests` runs `cargo nextest run --workspace --tests --locked` with no `env: DATABASE_URL`. Only the migrate step (`:567`) and the sqlx-check step (`:577`) set it, at step scope. The workflow-level `env` has no DATABASE_URL, and `.env` is gitignored and absent in CI.
  - Every DB test opens with `std::env::var("DATABASE_URL").ok()?` and returns `Ok(())` when it is unset. Examples: `crates/app/tests/jobs.rs:19`, `services/worker/tests/automation.rs:27`, `crates/http-api/tests/extraction_manual_values.rs:14`.
  - Log of main run 38097154018, job 114345331715: `856 tests run: 856 passed, 3 skipped` in **4.848 s**. DB tests show as PASS in ~10 ms, for example `deepref-app::jobs expired_leases_are_fenced_and_retries_reach_dead_at_max_attempts [0.010s]`, `deepref-worker::durable_processing claims_and_doi_leases_have_owner_expiry_and_attempts [0.006s]`, and `deepref-http-api::screening_postgres ... [0.013s]`. The slowest test in the whole run took 1.08 s.
  - History: `git log -S DATABASE_URL -- .github/workflows/ci.yml` shows only 429f68f and 13024c8. The pre-13024c8 workflow also lacked DATABASE_URL on the test step.
- **Current behaviour:** About 30k LOC of PostgreSQL, HTTP and worker integration tests pass vacuously on every PR. The required `E2E` check is green whatever the persistence layer does. That covers job leasing and fencing, migration data tests, screening transitions, AI-first blinding, project deletion and assistant runs.
- **Why it matters:** These are the app's real risks: durable jobs, double execution, migrations on realistic data, and cross-project boundaries. Tests for them exist, look rigorous, and give a green badge, but they never execute. They may already be broken locally too (I could not verify; see Open Questions).
- **Root cause:** The test helpers treat a missing DB as "skip" instead of "fail". CI and local differ: `justfile` has `set dotenv-load`, so `.env` supplies DATABASE_URL locally but not in CI. Nothing asserts that DB tests actually ran.
- **Proposed direction:**
  1. Add `DATABASE_URL: postgres://postgres:postgres@127.0.0.1:5432/deepref` at job level in `full-stack`.
  2. Add a guard. Option A: an env var such as `DEEPREF_REQUIRE_DB=1`, which makes the helper `panic!` instead of returning `None`. Option B: a final step asserting a minimum run time or count for `binary_id(deepref-app::*)`.
  3. Use `--profile ci`, which already exists in `.config/nextest.toml` and is unused.
  4. Drop `--tests` from `rust/test` or from `full-stack`. Today both compile and run the same binaries.
  5. Expect a first wave of real failures. Budget for it.
- **Verification:** Re-run full-stack. The jobs tests should take at least 100 ms and the summary time should rise from ~5 s to minutes. Then temporarily invert `leased_until > now()` in `complete_job` and confirm `jobs.rs` fails in CI.

### F2. A job that crashes the worker is reclaimed forever, ignores `max_attempts`, and takes the worker pool down each time (poison pill)
- **Category:** Defect (runtime and ops). **Severity:** P2. **Confidence:** High for the code path, Medium for the likelihood of a crashing job.
- **Evidence:**
  - `crates/app/src/jobs.rs:60-67`: `recover_expired_jobs` sets `state='queued'` for any expired `running` job with no `attempts < max_attempts` check.
  - `CLAIM` (`:12-28`) does not filter on attempts either. Only `fail_job` (`:153-185`) moves a job to `dead`.
  - `services/worker/src/lib.rs:124-133`: when a spawned job task ends in a `JoinError` (panic), the loop sets `loop_error` and exits, and the whole worker process stops.
  - The PDF parse runs in-process through PDFium under a global mutex (`crates/documents/src/parser.rs:117-121`).
- **Current behaviour:** A job that panics, or kills the process (OOM, PDFium crash on a hostile PDF), never reaches `fail_job`. After the 60 s lease it is requeued and claimed again, with `attempts` growing past `max_attempts`, and it crashes the next worker. That is a fleet-wide crash loop: all projects' queues stall.
- **Why it matters:** Availability of all background work depends on one bad input. `/health/dependencies` shows only "stale queue", and nothing names the job.
- **Root cause:** Recovery and dead-lettering are separate code paths, and the attempt budget is enforced only on the graceful-failure path.
- **Proposed direction:**
  - In `recover_expired_jobs`, set `state = CASE WHEN attempts >= max_attempts THEN 'dead' ELSE 'queued' END` and record `last_error='lease expired'`.
  - In the worker, catch a task panic per job (`JoinError::is_panic`), call `fail_job`, and keep the loop alive.
  - Add a DB test: claim, expire, repeat until max, and assert the job is `dead`.
- **Verification:** Write that new `jobs.rs` test. It fails today and passes after the fix, but it only runs once F1 is fixed.

### F3. Correlation IDs are decorative, failing jobs are not logged, and there is no HTTP access log at default level
- **Category:** Behaviour-preserving (observability). **Severity:** P2. **Confidence:** High.
- **Evidence:**
  - `crates/http-api/src/error.rs:157`: `correlation_id: Some(Uuid::new_v4())`, a fresh random value per error, logged nowhere.
  - `crates/http-api/src/routes/mod.rs:305-319`: the middleware stores the header id in request extensions. Nothing reads it, and it is not added to any span.
  - The web client prefers the body id (`custom-fetch.ts:36-38`). So the id a user reports matches nothing in any log, and it differs from the `x-correlation-id` response header.
  - `TraceLayer::new_for_http()` defaults are at DEBUG, and the default `RUST_LOG` is `info` (`.env.example`, Dockerfile `ENV RUST_LOG=info`).
  - `services/worker/src/lib.rs:255-270`: the `Err(error)` branch calls `fail_job(..., &error.to_string(), ...)` without any `tracing` call.
  - No metrics crate, no OTel propagator (`crates/telemetry/src/lib.rs`), and `current_trace_id()` is unused.
- **Current behaviour:** An operator cannot answer "which request failed", "which job failed and why" (beyond querying `jobs.last_error` by hand), or "is this caused by the deploy". There is no request rate, latency, error rate, queue depth or LLM latency metric. There is also no build SHA on log lines: `DEEPREF_BUILD_SHA` is set in the image, and I found no reader of it in the sources.
- **Proposed direction:**
  - Use the extension's correlation id in `ApiError` (through a task-local or `Extension`) and record it as a span field. Configure `TraceLayer` with an INFO `make_span_with` that includes `correlation_id`, the route template and the status.
  - Persist `correlation_id` into the `jobs` row (or payload) and open a `job` span with `job_id`, `kind`, `project_id`, `attempt` and `correlation_id` in `process_claimed_job`.
  - `warn!` on job failure and `error!` on dead-lettering.
  - Add a small RED metrics set (axum-prometheus or an OTel meter) plus queue gauges taken from the `/health/dependencies` query.
  - Put `service.version = DEEPREF_BUILD_SHA` on the resource.
- **Verification:**
  - Unit test: an error response body id equals the request header id.
  - Integration: force a job failure and assert a log line with `job_id`.

### F4. PRs touching only `packages/ui` skip all web checks, and the UI package's own tests never run in CI
- **Category:** Defect (CI gap). **Severity:** P2. **Confidence:** High.
- **Evidence:**
  - The `ci.yml:48-54` `web` filter is `apps/web/**`, root `package.json`, `pnpm-lock.yaml`, `pnpm-workspace.yaml` and `ci.yml`. There is no `packages/**`.
  - The `containers` filter (`:56-68`) also lacks `packages/**`, yet `apps/web/Dockerfile` copies `packages/ui`.
  - The `web` job runs `pnpm --filter @deepref/web test:coverage` only. `pnpm --filter @deepref/ui test` (5 specs) and `packages/ui` `test:visual` appear in no workflow.
  - Example commit touching only packages/ui: d8e3ce7 ("upgrade UI Vitest to v5").
- **Current behaviour:** On a ui-only PR, Web lint, check, test and build, browser e2e and the Docker build all "skip-succeed". Even on push to main, the UI vitest suite never runs.
- **Proposed direction:** Add `packages/**` to the `web` and `containers` filters, and run `pnpm -r run test` (or add `--filter @deepref/ui test`) in the Web job.
- **Verification:** Open a no-op PR touching `packages/ui/src/index.ts` and confirm the Web steps execute.

### F5. There is no real end-to-end test, so the API ↔ SPA contract is verified only by type generation
- **Category:** Architecture (test strategy). **Severity:** P2. **Confidence:** High.
- **Evidence:** `playwright.config.ts` `webServer` runs `npm run build && npm run preview` with no API. `tests/visual/api-fixture.ts` serves canned DTOs. The `E2E` required check combines F1 (vacuous DB tests) with mocked browser tests.
- **Current behaviour:** Generated types catch shape drift. They do not catch semantics: status codes, error codes the UI branches on (409 `calibration_*`, `ai_budget_exceeded`), pagination cursors, SSE framing for assistant runs, or the CORS and `/api` prefix stripping in Caddy and Vite.
- **Proposed direction:** Add one thin, real smoke lane in `full-stack`:
  1. Start `deepref-server all` with compose Postgres and `just seed`.
  2. Build the web app with `API_PROXY_TARGET`.
  3. Run 3–5 Playwright journeys tagged `@real`: create project, import a small RIS, screen one record, open settings, and an SSE run with a stub AI provider.
  Keep the visual suite mocked.
- **Verification:** The lane fails when a route's status code changes, for example 409 → 400 on a calibration refusal.

### F6. Three of four scheduled quality lanes have failed every run since they were added, and nobody noticed
- **Category:** Defect (CI). **Severity:** P3 (no production impact; wasted signal). **Confidence:** High.
- **Evidence (`gh run list`):** ts-mutants failure=4, fuzz failure=4, benchmarks failure=4, and none ever succeeded.
  - **fuzz:** `error: current package believes it's in a workspace when it's not`. Root `Cargo.toml` has no `exclude = ["fuzz"]`, and `fuzz/Cargo.toml` has no `[workspace]`.
  - **benchmarks:** `error: Unrecognized option: 'save-baseline'` / `bench failed ... -p deepref-ai --lib`. `cargo bench --workspace -- --save-baseline main` passes the Criterion flag to libtest harnesses (`benchmarks.yml:58`).
  - **ts-mutants:** earlier runs failed on unresolvable action SHAs, re-pinned in afad978 on Oct 9 and not re-run since. It also uses `node-version-file: .mise.toml`. As far as I know setup-node does not parse mise TOML: it would take the first line matching its regex (`[tools]`) as the version. Medium confidence; it is the only workflow not using `node-version: 24.21.0`.
  - **rust-mutants:** "succeeds" only because of `|| true`.
  - **publish-images:** has never run (F8).
- **Proposed direction:** Either fix the lanes (`exclude = ["fuzz"]`, `cargo bench --workspace --benches -- --save-baseline main`, `node-version: 24.21.0`) or delete them (§5). Add failure notification for scheduled workflows: an issue-on-failure step, or at least watch the Actions tab.
- **Verification:** `workflow_dispatch` each lane once.

### F7. Mutation testing shows weak assertions in validation logic, and the report is never acted on
- **Category:** Behaviour-preserving (test quality). **Severity:** P3. **Confidence:** High (from the run artifact).
- **Evidence:**
  - Artifact `rust-mutants-report-107455328b...` (run 37195437668): 332 missed, 250 caught.
  - Survivors include whole-function replacements:
    - `appraisal.rs:184 validate_definition_resource -> Ok(())`
    - `appraisal.rs:247 validate_judgment -> Ok(())`
    - `appraisal.rs:267 validate_answer_schema -> Ok(())`
    - many `|| → &&` in `validate_definition_semantics`
    - `ScreeningQueueStatus::parse -> None`
    - 65 survivors in `automations.rs`
  - `validate_definition_resource` is called at an HTTP boundary (`crates/http-api/src/routes/study.rs:1094`) whose tests are DB-backed, so they skip (F1).
  - `.cargo/mutants.toml` does not set `test_workspace = true`, so only same-crate tests are tried.
  - Default dispatch input `package: deepref-core` targets a 230-LOC crate.
- **Proposed direction:**
  - Add table-driven unit tests for the appraisal validators and the `parse`/`as_str` round-trips. They are cheap and high-yield.
  - After F1 is fixed, try `test_workspace = true` with DB in the mutants job, or keep it package-scoped and treat survivors as a to-do list.
  - Publish the missed count as a job summary and track the trend.
- **Verification:** Re-run mutants on `deepref-application`; the missed count should fall well below 300.

### F8. The release pipeline is unexercised, unaffected by CI results, and likely to time out on arm64
- **Category:** Defect / risk (release). **Severity:** P2. **Confidence:** Medium.
- **Evidence:**
  - `publish-images.yml` has zero runs.
  - It triggers on any `v*` tag with no dependency on CI success for that SHA.
  - It builds `linux/amd64,linux/arm64` (`docker-bake.hcl` `release.platforms`) on an amd64 runner under **QEMU** with `timeout-minutes: 45`.
  - `cache-from` names only `*-amd64` scopes, so the arm64 Rust build is cold and emulated. CI deliberately validates arm64 on a native `ubuntu-24.04-arm` runner instead.
  - `${{ github.event.inputs.tag }}` is interpolated directly into a `run:` script (`:41`). This is template injection, limited to people with dispatch rights. zizmor would flag it.
  - Every tag also pushes `:latest`, including pre-releases.
  - Images are never started in CI (`output=type=cacheonly`). No smoke test proves `deepref-server --help` runs or that `libpdfium.so` loads in the slim runtime.
- **Proposed direction:**
  - Build per-arch natively (matrix of amd64 and `ubuntu-24.04-arm`) with `cache-from` the matching scope, then merge manifests.
  - Gate on a successful CI run for `github.sha`.
  - Pass inputs through `env:`.
  - Add a post-build smoke step: `docker run --rm img migrate --help` or `--print-openapi`, plus a tiny PDFium load (`dlopen`) self-test.
- **Verification:** Dispatch with tag `edge` on a fork and time it.

### F9. Readiness checks a hard-coded schema version 10 while the code needs 58, and the worker has no health surface
- **Category:** Defect (ops). **Severity:** P2. **Confidence:** High.
- **Evidence:**
  - `crates/http-api/src/routes/health.rs:95-114`: `if schema_version < 10` and `required_schema_version: 10`. The latest migration is `0058_assistant_runs_no_suspended.sql`.
  - The API does not compare the migrator's embedded latest version with the DB at startup. The worker checks nothing.
  - `process-compose.yaml` gates on `/health/live`.
- **Current behaviour:** If you deploy API N against DB N-k (migrate forgotten or failed), readiness passes, traffic arrives, and individual queries fail with 500s. The worker fails per job. An N-1 worker running beside an N schema (for example after 0058 removed `suspended`) is not detected either.
- **Proposed direction:**
  - Derive the required version from `deepref_app::MIGRATOR.iter().map(|m| m.version).max()`.
  - Have readiness fail when the DB is behind, and log a warning when it is ahead (N+1 schema with an N binary, which is OK only under an expand/contract policy).
  - Give the worker either a tiny `/health/live` listener or a heartbeat row such as `worker_heartbeats(owner, seen_at)` that `/health/dependencies` reports.
  - Document the expand/contract rule for migrations.
- **Verification:** A test asserting the readiness requirement equals the max embedded migration.

### F10. `APP_ENV` defaults to `local` when unset, which disables every hosted fail-fast guard
- **Category:** Defect (config safety). **Severity:** P2. **Confidence:** High.
- **Evidence:**
  - `crates/config/src/lib.rs:41`: `Environment::parse(get("APP_ENV").as_deref().unwrap_or("local"))`.
  - With `local`, `DATABASE_URL` silently defaults to `127.0.0.1:5432` (`database.rs:38-40`), `API_CORS_ALLOW_ANY=true` is allowed, CORS defaults to localhost origins, and `DOCUMENT_STORAGE_BACKEND=local` is allowed.
  - The Dockerfiles do not set `APP_ENV`.
  - `apps/server/src/main.rs` also calls `load_dotenv_files()` in production, which reads `.env.local` and `.env` from the working directory.
- **Current behaviour:** A container started without `APP_ENV` runs in "local" mode. A misconfigured deploy can come up with permissive CORS and local-disk document storage on an ephemeral container filesystem, and uploads are lost on restart.
- **Proposed direction:**
  - Default to `production` in the binary and have `.env.example` and process-compose set `APP_ENV=local` (they already do).
  - Or set `ENV APP_ENV=production` in the runtime images.
  - Skip dotenv loading unless `APP_ENV=local`.
- **Verification:** A config test: no APP_ENV plus no DATABASE_URL gives `Err(Missing)`.

### F11. CI wastes compute and doubles some work
- **Category:** Behaviour-preserving (CI cost). **Severity:** Opportunity. **Confidence:** High.
- **Evidence:**
  - `rust/test` runs `--tests` and `full-stack` runs `--tests` again, compiling the same test binaries twice.
  - `ai-eval-security-gates` re-runs `eval_gate` and `prompt_injection_regressions`, which `rust/test` already runs. It is not a required check.
  - `duplication` (jscpd) is report-only and not required.
  - The `docs` job installs ripgrep and does not use it.
  - The `[profile.ci]` nextest profile is unused.
- **Proposed direction:**
  - Make `rust/test` run `--lib --bins --doc` only, and give `full-stack` everything under `--tests` with DB.
  - Delete `ai-eval-security-gates`, or make it a nextest filter inside `rust/test`.
  - Move jscpd to the weekly lane.
- **Verification:** CI minutes per PR drop. Required checks are unchanged.

### F12. API graceful shutdown has no deadline, so open SSE observers can hold termination
- **Category:** Defect (runtime). **Severity:** P3. **Confidence:** Medium.
- **Evidence:**
  - `crates/http-api/src/lib.rs:76-79`: `with_graceful_shutdown(shutdown)` waits for all connections.
  - `routes/assistant/run_events.rs` streams SSE with keep-alive and a 500 ms DB poll per observer.
  - `SHUTDOWN_DEADLINE_SECS` is applied only to the worker and to OTel.
- **Current behaviour:** On SIGTERM the API stops accepting connections but keeps serving long-lived SSE until the orchestrator SIGKILLs it (process-compose 40 s, Kubernetes default 30 s). Deploys are slower, and the kill is ungraceful.
- **Proposed direction:** Wrap serving in `tokio::time::timeout(shutdown_deadline, ...)` after the signal, and end SSE streams on the shutdown notification. Clients already resume with `after_seq`.
- **Verification:** A local test: open an SSE stream, send SIGTERM, and expect the process to exit within the deadline.

### F13. Local developer setup has hidden prerequisites and unsafe sharing with the dev DB
- **Category:** Behaviour-preserving (DX). **Severity:** P3. **Confidence:** High.
- **Evidence:**
  - README and `docs/local-development.md` list only "mise, Docker".
  - Tools the documented commands need but `.mise.toml` does not pin:
    - `just verify` needs `sentrux`.
    - `just test-unit` and `just test-integration` need `cargo-nextest`.
    - `just test-coverage-rust` needs `cargo-llvm-cov`.
    - `just sqlx-*` and CI need `sqlx-cli`.
    - `deny.toml` needs `cargo-deny`.
    These happen to be installed globally on this machine (`~/.cargo/bin`, `~/.local/bin/sentrux`).
  - PDF parsing needs libpdfium (`PDFIUM_LIBRARY_PATH`, `crates/documents/src/parser.rs:105`). The local docs never mention it, so `just dev` cannot parse PDFs out of the box. Failure shows up lazily as job errors, not at worker startup.
  - `just test-integration` runs DB tests against the developer's **dev database**: the same compose volume and `.env` DATABASE_URL. Tests like `jobs.rs` call `queue.claim()` globally and can claim real queued dev jobs while `just dev`'s worker competes for test fixtures.
  - `DATABASE_TEST_MUTEX` is an in-process mutex. Nextest runs each test in its own process, so it serialises nothing (the nextest config admits this for `review_runs`). The `database` test group covers only `deepref-app`. Worker and http-api DB tests are unbounded.
  - Logs are always JSON (`crates/telemetry/src/lib.rs:56-75`), which is hard to read under process-compose.
- **Proposed direction:**
  - Pin `cargo:cargo-nextest`, `cargo:cargo-llvm-cov`, `cargo:sqlx-cli` and `cargo:cargo-deny` in `.mise.toml`, and either pin sentrux or remove it from `verify`.
  - Add a `pdfium` mise or just recipe (same URL and checksum as the Dockerfile).
  - Make `test-integration` use a separate database (`deepref_test`, created by the recipe), and use `sqlx::test`-style per-test databases or schemas.
  - Pretty logs when `APP_ENV=local`.
- **Verification:** A fresh clone in a container with only mise and Docker: `just bootstrap && just verify && just test-unit` succeeds.

### F14. Documentation accuracy and agent-guidance drift
- **Category:** Behaviour-preserving (docs). **Severity:** P3. **Confidence:** High.
- **Evidence:**
  - `AGENTS.md` links all point to `file:///home/luzes/.t3/worktrees/ambient-scribes/t3code-729f9577/...`, a path that does not exist. The repo-relative files do exist.
  - `AGENTS.md` describes the monorepo as `apps/web, packages/ui, packages/core`. There is no `packages/core`.
  - `AGENTS.md` §3 "Verification" lists only `pnpm check/test/lint`, with no Rust gates, for a repo that is mostly Rust.
  - `scripts/check-docs.sh` does not scan `AGENTS.md`, and would fail on the `file://` links if it did.
  - `docs/api.md:20` says "Hosted migrations run as the Helm/Argo PreSync Job". The Helm chart was deleted in e70e800.
  - `.jscpd.json` still ignores `infra/**` and `charts/**`.
  - `docs/README.md` (6 lines) does not index the runbook, the deployment guide, the ADRs or the PRDs.
  - The runbook and deployment guide hard-code ~27 `file.rs:NN` line references each. They will rot quickly and nothing checks them.
  - `CONTRIBUTING.md` says CI runs "on pull requests to main", but security runs on schedule too, and the fuzz, mutants and bench lanes are not mentioned.
  - `skills-lock.json` locks 6 skills, but `.agents/skills` holds 10 (orval, runed, shadcn-svelte-extras and svelte-query are unlocked).
  - `docs/adr/0005/` is a directory of benchmark artifacts (`workload.py`, `results.json`) sitting beside numbered ADR files.
- **Proposed direction:**
  - Make AGENTS.md links repo-relative, add the Rust gates, and add AGENTS.md to check-docs.
  - Fix api.md.
  - Index the docs.
  - Replace line-number references with symbol names.
  - Regenerate skills-lock.
- **Verification:** `bash scripts/check-docs.sh` with AGENTS.md included passes.

### F15. Repo hygiene: ~64 GB of untracked worktrees and targets inside the repo, Docker context leakage, and stale leftovers
- **Category:** Behaviour-preserving (DX). **Severity:** P3. **Confidence:** High for size, Medium for the Docker impact.
- **Evidence:**
  - Untracked trees inside the repo: `.worktrees/` 41 GB (81 MB excluding target and node_modules), `.claude/worktrees/` 1.6 GB, `.claude/targets/` about 23 GB (int 12 G, pr1 4.8 G, pr3a and pr3b 3 G each). `/home` reached 100% during this review.
  - Ignored only through `.git/info/exclude`, not `.gitignore`: `.worktrees/` and `.claude/worktrees/`. `.claude/` is ignored somewhere; `git status --ignored` lists it.
  - `.dockerignore` excludes `.repos` but **not** `.worktrees` or `.claude`. A local `just docker-build` sends about 225 MB of nested sources (181 nested `Cargo.toml`) into the context. `COPY . .` in the planner stage then copies them, and cargo-chef's recipe is likely to pick up the nested manifests and bust the cache. CI checkouts are unaffected.
  - Tools that read only `.gitignore` (prettier, eslint `includeIgnoreFile`, possibly sentrux run at `.`) do not see `.git/info/exclude`.
  - Leftovers: `observability/` contains only `synthetics/node_modules`; `examples/{infra,services}` are empty untracked directories; `apps/web/src/lib/vitest-examples/greet.*` is scaffold.
  - `dependabot.yml` docker ecosystem scans `/` and `/apps/web`, but the Rust Dockerfile lives in `docker/`, so its digests are never bumped.
- **Proposed direction:**
  - Move worktrees and targets outside the repo (`../deepref-worktrees`), or add `.worktrees/`, `.claude/` and `examples/` to `.gitignore` and `.dockerignore`.
  - Delete the leftovers.
  - Add a dependabot docker entry for `/docker`.
- **Verification:** `docker buildx bake --print` and context size; `du -sh` of the repo.

### F16. A new migration (0056) fail-closes AI automation, and there is no supported way to recalibrate
- **Category:** Product / ops. **Severity:** P2 for anyone running the app. **Confidence:** High, because the repo's own doc says "Gap, verified".
- **Evidence:** `docs/deployment-recalibration.md`: "there is no HTTP endpoint or CLI that records calibration bundles. The only callers of `insert_review_calibration_bundle` are tests". The migration is already on main.
- **Current behaviour:** After deploy, automation admission is refused (409 `calibration_incompatible`) until someone writes calibration rows with hand-made SQL.
- **Proposed direction:** Add a `deepref-server recalibrate` subcommand, or an admin endpoint that uses `preview_*_identity` and `insert_review_calibration_bundle`, before any release tag. Until then, put this in release notes.
- **Verification:** Run the subcommand against a migrated seed DB and confirm `check_admission` returns admitted.

---

## 4. Tests that give false confidence

1. **The DB-backed tests in CI (F1).** About 30k LOC that pass in ~10 ms by returning early, which is the largest single source of false confidence. This includes the well-written `jobs.rs` fencing tests, the migration data tests (`migration_0013..0016`, `calibration_stage_migration`), the worker `durable_processing`, `assistant` and `ai_first` tests, and all `*_postgres.rs` HTTP tests.
2. **Source-grep "tests":**
   - `services/worker/tests/crash_boundaries.rs` finds the substring `enqueue_child(&mut tx` before `complete_item_and_claim(\n        &mut tx,...` in `store.rs`. Reformatting the code breaks it, while a logic bug in the transaction passes.
   - `persistence_boundaries.rs` checks that `reconciler.rs` does not contain the token `jobs`.
   - `global_throttle.rs` asserts the source contains `"FOR UPDATE"`.
   - `reconciliation.rs` checks migration text for index names.
   - `crates/http-api/tests/migration.rs::migrations_are_append_only_and_define_durability_contract` checks only that migration files contain words. It does not check append-only behaviour. Nothing in CI prevents editing an applied migration. sqlx checksums would catch that only at deploy time.
   These are architecture lints, not behaviour tests. Rename or move them to xtask, and don't count them as integration coverage.
3. **"Browser E2E" and the required `E2E` check:** fully mocked API plus vacuous DB tests. They are good UI regression tests but do not exercise end to end.
4. **"AI evaluation and security gates":** `eval_gate.rs` evaluates a checked-in JSON of predictions (`fixtures/evals/reviewed-small-v1.json`). It tests the gate arithmetic, not the model or the prompts. The name suggests model-quality gating.
5. **Mutation survivors (F7)** show the appraisal validators can be replaced with `Ok(())` without any failing test.
6. **Health `worker_state` tests** use 301 s and 12 s but never the 300 s boundary, so `>` → `>=` survives. Minor.
7. **AI provider failure paths** are tested only as pure classification functions (`openai_compat.rs` `is_retryable` and `subscription_limit_reached`). There is no HTTP-level fake (wiremock or a local stub) for timeouts, truncated SSE, malformed JSON or connection reset. Cassettes cover the happy-path replay.

---

## 5. Deletion candidates

| Item | Reason |
|---|---|
| `ci.yml` job `ai-eval-security-gates` | Duplicates `rust/test` and is not required. |
| `ci.yml` `--tests` in `rust/test` *or* `full-stack` | Double compile and double run (F11). |
| `benchmarks.yml`, `fuzz.yml`, `ts-mutants.yml` | Fix them (one-line fixes, F6) or delete them. They have never produced a result. |
| `duplication` job (jscpd on PR) | Report-only and unread; move it to weekly or drop it (Rust duplication is also visible through sentrux and clippy). |
| `docs` job `apt-get install ripgrep` | Unused. |
| sentrux (local `just verify` only, not in CI, not pinned) | Either put it in CI or drop it. Today it is a local-only gate a new contributor cannot install from the docs. |
| `cargo xtask doctor` | Counts files and re-runs boundaries; low value. Fold it into `just verify`. |
| Duplicate OpenAPI entrypoints | `deepref-http-api --bin openapi`, `deepref-server --print-openapi`, `pnpm generate:api` and `cargo xtask generate` all exist. Keep one binary path and one wrapper. |
| `GET /health` (deprecated) | Still routed and documented; schedule removal. |
| `observability/` (node_modules only), `examples/` (empty), `apps/web/src/lib/vitest-examples/` | Leftovers. |
| `.jscpd.json` ignores for `infra/**` and `charts/**`; `.dockerignore` terraform block | Refer to deleted directories. |
| Handoff docs (`docs/*-handoff.md`, `ui-review.md`, `rete-workflow-migration.md`) | Agent work-plans, not reference docs. Archive them or mark their status; `backend-ui-rebalance-handoff.md` proposals (dropping pdfjs-dist) are unimplemented and undated. |
| `docs/adr/0005/` artifact directory | Move it to `docs/quality/` or a bench folder. |
| `.env.local` `ZAI_*` legacy config, `DEEPREF_AI_PROVIDER=zai` path | The retired provider is kept "for emergencies". Consider removing it with a deprecation date. |

---

## 6. Open questions

1. Do the DB-backed integration tests pass today? I could not run them: disk exhaustion, and the baseline cargo process held `target/`. Suggested check:
   ```
   docker run -d --rm -p 127.0.0.1:55439:5432 -e POSTGRES_PASSWORD=postgres -e POSTGRES_DB=deepref pgvector/pgvector:0.8.7-pg17-bookworm
   DATABASE_URL=postgres://postgres:postgres@127.0.0.1:55439/deepref SQLX_OFFLINE=true cargo nextest run --workspace --tests
   ```
   Run it against a throwaway DB, **not** the dev DB.
2. What is the intended production topology (Kubernetes, compose on a VM, a PaaS)? Since the chart was removed, nothing in the repo defines how migrate, API and worker are sequenced, how many workers run, or where OTLP goes.
3. Is the `all` role used in production? If so, both roles log under service name `deepref-api` (`apps/server/src/lib.rs:31`), and a worker panic stops the API.
4. Is there an intended alerting stack? There are no metrics, so alerts could come only from logs. Decide whether `/health/dependencies` is the intended probe for an external monitor. It is unauthenticated and accepts arbitrary `project_id`; flag to the security reviewer.
5. Should `serve` write to the DB at startup (`ensure_default_model_routes`)? With N replicas it races, and it contradicts "serve never mutates; migrate is explicit" in spirit.
6. Is anyone reading the weekly Rust mutants and risk artifacts? If not, consider a job summary or an issue.
