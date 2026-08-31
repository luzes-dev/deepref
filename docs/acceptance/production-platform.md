# Production platform acceptance

This register records the sixteen current acceptance areas for the supported PostgreSQL-backed architecture. A source file or local static test is not apply-time evidence. Status is current as of 2026-08-18 and must be updated only when the named evidence has actually been produced and reviewed.

Status terms: **Pending** means no sufficient evidence; **Partial** means source/static evidence exists but the full criterion is not demonstrated; **Verified locally** means the complete locally provable criterion passed in this tree; **Accepted** requires reviewed environment/drill evidence.

## AC-01 — Existing checks remain green

- **Criterion:** Rust, web, API-codegen, documentation, infrastructure, and Helm checks remain green.
- **Automated evidence path/command:** `.github/workflows/ci.yml`; `cargo fmt --all --check`, locked Rust checks, `pnpm generate:api:check`, web checks, `bash scripts/check-docs.sh`, and `bash scripts/helm-check.sh`.
- **Manual drill/apply-time evidence:** Successful required CI checks on each promoted source tree.
- **Prerequisites:** Pinned tools, dependencies, Docker for integration, and Playwright browser.
- **Current verification status:** **Partial.** Final verification for this continuation pass is recorded in the handoff.

## AC-02 — New-developer local workflow

- **Criterion:** A new developer installs tools, starts PostgreSQL, and runs web/API/worker through mise/Just/Process Compose.
- **Automated evidence path/command:** `.mise.toml`, `justfile`, `process-compose.yaml`, `infra/local/compose.yaml`; `mise exec -- just bootstrap && mise exec -- just dev`.
- **Manual drill/apply-time evidence:** Fresh-machine walkthrough showing PostgreSQL healthy, migrations successful, roles running, and the named PostgreSQL volume reset.
- **Prerequisites:** mise, Docker Compose, network access for first install, and free local ports.
- **Current verification status:** **Partial.** Source workflow exists; fresh-machine evidence is not retained.

## AC-03 — Durable job correctness

- **Criterion:** Claims, lease renewal/recovery, stale-owner rejection, retry/dead persistence, stable dedupe, bounded concurrency, reconciliation, and transaction-coupled enqueue are covered.
- **Automated evidence path/command:** `crates/application/src/jobs.rs`, `crates/postgres/src/jobs.rs`, `services/worker/src/{lib,store,reconciler}.rs`, migration 0008; `cargo test --workspace --locked`.
- **Manual drill/apply-time evidence:** Disposable PostgreSQL fixture with concurrent claims, expiry/recovery, retries, terminal failure, duplicate enqueue, crash recovery, and later reconciliation.
- **Prerequisites:** Disposable PostgreSQL and deterministic concurrent fixture.
- **Current verification status:** **Partial.** Source and integration fixture are present; live database evidence is environment-dependent.

## AC-04 — Ingestion preservation

- **Criterion:** Cached/fetched facts, discovered children, citations, authoritative domain events, job enqueue, and completion remain durable after the queue cutover.
- **Automated evidence path/command:** `services/worker/src/processor.rs`, `services/worker/src/store.rs`, ingestion routes, and PostgreSQL migrations; locked worker tests.
- **Manual drill/apply-time evidence:** Run a representative recursive import, interrupt at lease/commit boundaries, reconcile, and compare deterministic row counts and UUID facts.
- **Prerequisites:** Provider fixture or approved test endpoint, disposable database, and fault-injection harness.
- **Current verification status:** **Partial.** Transactional source paths exist; no running crash drill is retained.

## AC-05 — PostgreSQL graph semantics

- **Criterion:** UUID reports/project membership/citations produce deterministic bounded nodes and edges; identifier-free reports are included; metrics match the legacy fixture semantics.
- **Automated evidence path/command:** `crates/postgres/src/graph.rs`, `crates/postgres/tests/graph.rs`, migration 0008; `cargo test -p deepref-postgres --test graph --locked` with `DATABASE_URL`.
- **Manual drill/apply-time evidence:** Seed the exact fixture, compare node/edge/internal/outbound/rank values, repeat import, and verify later recomputation freshness.
- **Prerequisites:** Disposable PostgreSQL with migrations 0001 through 0010 applied.
- **Current verification status:** **Partial.** Deterministic fixture is present; database execution is pending environment access.

## AC-06 — Runtime roles and shutdown

- **Criterion:** `serve` is HTTP-only, `worker` runs PostgreSQL jobs, and `all` coordinates both with bounded JoinSet/Semaphore concurrency and clean shutdown.
- **Automated evidence path/command:** `apps/server`, `services/worker`, `process-compose.yaml`, Docker targets, and chart deployments.
- **Manual drill/apply-time evidence:** Start each role, submit work, stop it during active jobs, and verify lease recovery and no duplicate process ownership.
- **Prerequisites:** PostgreSQL, built `deepref-server`, and Process Compose or Kubernetes.
- **Current verification status:** **Partial.** Source/configuration is present; live role/shutdown evidence is pending.

## AC-07 — Failed migration blocks rollout

- **Criterion:** A failed PostgreSQL migration blocks rollout while the previous application continues serving.
- **Automated evidence path/command:** `charts/deepref/templates/migration-job.yaml`, chart migration tests, and migration failure runbook.
- **Manual drill/apply-time evidence:** Apply an intentionally failing additive migration in a safe environment and retain the PreSync failure, old ReplicaSet health, and forward-fix record.
- **Prerequisites:** Deployed Argo/chart and safe test schema.
- **Current verification status:** **Partial.** Hook ordering is source-tested; no live Argo failure drill is retained.

## AC-08 — Graph freshness and recomputation

- **Criterion:** Fresh imports and later ingestions refresh project metrics; recompute updates metric snapshots and graph freshness coherently.
- **Automated evidence path/command:** migration 0008, legacy importer, worker persistence path, projection route, and `crates/postgres/tests/graph.rs`.
- **Manual drill/apply-time evidence:** Import twice, enqueue recomputation, compare metric revision/timestamp/snapshot state, and verify no stale external projection is required.
- **Prerequisites:** Disposable PostgreSQL and representative import fixture.
- **Current verification status:** **Partial.** Source path and assertions exist; live database execution is pending.

## AC-09 — RDS RPO/RTO

- **Criterion:** Production RDS Multi-AZ failover and PITR meet the declared RPO of five minutes and RTO of sixty minutes.
- **Automated evidence path/command:** `infra/modules/rds/**`, `infra/environments/production/main.tf`, module tests, and the RDS runbook.
- **Manual drill/apply-time evidence:** Approved failover and isolated PITR restore with clock definitions, data invariants, and measured RPO/RTO.
- **Prerequisites:** Applied production RDS, healthy recovery points, and data/platform owners.
- **Current verification status:** **Pending apply-time drill.**

## AC-10 — Exact signed digest promotion

- **Criterion:** Exact signed API, worker, web, and chart subjects progress through all environments without rebuild.
- **Automated evidence path/command:** Release/promote workflows, release schemas/fixtures, and CI scripts.
- **Manual drill/apply-time evidence:** One release manifest and environment locks with matching source tree, signatures, attestations, and deployed image IDs.
- **Prerequisites:** ECR accounts/roles, GitHub OIDC/protected environments, App-only GitOps branch, and OCI signing support.
- **Current verification status:** **Partial.** Workflow contracts exist; no hosted promotion evidence is retained.

## AC-11 — Unsigned/unapproved rejection

- **Criterion:** Unsigned or wrong-identity images are rejected by admission policy.
- **Automated evidence path/command:** `charts/deepref/templates/kyverno-verify-images.yaml`, chart tests, and immutable-image policy.
- **Manual drill/apply-time evidence:** Controlled staging admission denials followed by an approved signed image.
- **Prerequisites:** Installed Kyverno/controller, mirrored digest-only images, and private cluster access.
- **Current verification status:** **Partial.** Render/policy source exists; no server-side evidence exists.

## AC-12 — Access denial and no origin bypass

- **Criterion:** A nonmember is denied and no public AWS application origin exists.
- **Automated evidence path/command:** Cloudflare perimeter module, synthetic access canary, module tests, and static contracts.
- **Manual drill/apply-time evidence:** Member allow/nonmember deny, DNS/tunnel/JWT proof, AWS inventory, and synthetic alert evidence.
- **Prerequisites:** Applied global/per-environment infrastructure and test identities.
- **Current verification status:** **Pending apply-time drill.**

## AC-13 — GitOps rollback

- **Criterion:** A prior compatible release is restored through a protected GitOps PR.
- **Automated evidence path/command:** Rollback workflow and release-lock validators/fixtures.
- **Manual drill/apply-time evidence:** Staging then production rollback with same-migration guard, approvals, Argo sync, and health recovery.
- **Prerequisites:** GitOps history, compatible artifacts, protected App/environments, and Argo.
- **Current verification status:** **Partial.** Protected workflow contract exists; no hosted rollback evidence exists.

## AC-14 — Empty plan and healthy Argo

- **Criterion:** Follow-up OpenTofu plans are empty and every Argo application is healthy and synced.
- **Automated evidence path/command:** Infrastructure apply workflow and `mise exec -- just infra-validate`.
- **Manual drill/apply-time evidence:** Successful apply per root, exit code zero follow-up plan, drift review, and Argo status at the accepted GitOps revision.
- **Prerequisites:** Remote backends, protected OIDC roles, private EKS runner, GitOps branch, and Argo children.
- **Current verification status:** **Pending apply-time apply.**

## AC-15 — Supported observability

- **Criterion:** Active dashboards, alerts, runbooks, and health contracts describe PostgreSQL, durable jobs, API, worker, web, Cloudflare, and telemetry only.
- **Automated evidence path/command:** `observability/**`, active operations docs, `scripts/check-docs.sh`, and API-codegen checks.
- **Manual drill/apply-time evidence:** Alert delivery and incident response exercise against the supported roles and queue metrics.
- **Prerequisites:** Deployed telemetry and alert routing.
- **Current verification status:** **Partial.** Active source cleanup is in this tree; hosted alert delivery is pending.

## AC-16 — No supported self-host path

- **Criterion:** Documentation contains no supported hosted deployment path outside the OpenTofu/ECR/Helm/Argo workflow and describes Compose as disposable local PostgreSQL tooling only.
- **Automated evidence path/command:** `scripts/check-docs.sh` and active documentation scan.
- **Manual drill/apply-time evidence:** Documentation review confirms local-only Compose and hosted release ownership.
- **Prerequisites:** All documentation changes present; the historical production plan remains excluded and unchanged.
- **Current verification status:** **Partial.** Final documentation check is part of this continuation pass.

## Acceptance sign-off rule

Production acceptance requires every criterion to be **Accepted**, with evidence location, reviewer, and date recorded by the accountable owner. Local commands can close only locally provable portions. EKS, RDS, Argo, Cloudflare, GitHub, admission, and recovery drills remain pending until the required hosted prerequisites are deployed.
