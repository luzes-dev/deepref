# Production platform acceptance

This register maps `AC-01` through `AC-16` one-for-one, in order, to the sixteen acceptance bullets in `docs/production-platform-plan.html`. A source file or local static test is not apply-time evidence. Status is current as of 2026-08-05 and must be updated only when the named evidence has actually been produced and reviewed.

Status terms: **Pending** means no sufficient evidence; **Partial** means source/static evidence exists but the full criterion is not demonstrated; **Verified locally** means the complete locally provable criterion passed in this tree; **Accepted** requires reviewed environment/drill evidence.

## AC-01 — Existing checks remain green

- **Criterion:** Existing Rust and web checks remain green.
- **Automated evidence path/command:** `.github/workflows/ci.yml`; `mise exec -- just verify`, `mise exec -- just test-unit`, `mise exec -- just test-integration`, `mise exec -- just test-e2e`, and `mise exec -- just codegen-check`.
- **Manual drill/apply-time evidence:** Successful required CI checks on the exact source commits promoted through `development`, `staging`, and `main`; retain workflow URLs and source SHA/tree.
- **Prerequisites:** Pinned mise tools, JavaScript/Rust dependencies, Docker for integration, Playwright browser, and any test dependency images.
- **Current verification status:** **Partial.** On 2026-08-05, Rust format/clippy/all-target tests, web lint/check/unit/build, and codegen drift passed locally. Browser E2E could not start its preview listener because this execution sandbox denies local socket binding (`EPERM`), so the complete criterion and hosted required checks remain pending.

## AC-02 — New-developer local workflow

- **Criterion:** A new developer installs tools, starts dependencies, and runs every service through the documented mise/Just workflow.
- **Automated evidence path/command:** `.mise.toml`, `justfile`, `process-compose.yaml`, `infra/local/compose.yaml`, `scripts/bootstrap-local-nats.sh`; `mise trust && mise install && mise exec -- just bootstrap && mise exec -- just dev`.
- **Manual drill/apply-time evidence:** Fresh-machine walkthrough showing PostgreSQL/NATS/Neo4j healthy, migration and stream bootstrap successful, web/API/worker/projector running, then `just dev-down` and an explicitly approved disposable `just dev-reset` test.
- **Prerequisites:** mise, Docker Compose, network access for first tool/package/image install, local ports free, and a nonproduction machine.
- **Current verification status:** **Partial.** Source workflow exists; a fresh-machine end-to-end walkthrough has not been retained.

## AC-03 — Correctness coverage

- **Criterion:** Tests cover claims, lease recovery, rollback, idempotency, cached-work attachment, ingestion completion, event compatibility, and global throttling.
- **Automated evidence path/command:** `services/worker/tests/{durable_processing,reconciliation,crash_boundaries,global_throttle}.rs`, worker store tests, `crates/events/src/compatibility.rs`; `cargo test --workspace --all-targets --locked`.
- **Manual drill/apply-time evidence:** Transactional tests against disposable PostgreSQL/NATS exercising concurrent claims, expiry/recovery, cached DOI attachment across projects, rollback at failure points, logical idempotency, child-before-completion ordering, legacy event compatibility, and global throttle contention.
- **Prerequisites:** Disposable PostgreSQL/NATS fixtures and deterministic fault/concurrency harness.
- **Current verification status:** **Partial.** Several named tests are static source/schema assertions rather than full database/concurrency integration; complete behavioral evidence is missing.

## AC-04 — Worker crash boundaries

- **Criterion:** Crashing a worker at every fetch, commit, publish, and ACK boundary loses no child work and creates no duplicate logical facts.
- **Automated evidence path/command:** `services/worker/tests/crash_boundaries.rs`, `services/worker/src/{store,processor,outbox,reconciler}.rs`; intended command `cargo test -p deepref-worker --test crash_boundaries --locked`.
- **Manual drill/apply-time evidence:** Fault-injection run for each boundary with PostgreSQL/NATS, followed by reconciliation, child/outbox/event counts, deterministic-ID duplicate checks, and consumer convergence.
- **Prerequisites:** Real disposable PostgreSQL and JetStream, kill/fault harness, invariant queries, and repeatable dataset.
- **Current verification status:** **Partial.** The current crash-boundary test checks source ordering; it does not crash a running worker.

## AC-05 — Bounded DLQ delivery

- **Criterion:** Malformed and repeatedly failing events reach the DLQ after bounded delivery.
- **Automated evidence path/command:** `services/worker/tests/delivery_policy.rs`, `services/worker/src/delivery.rs`, `charts/deepref/templates/nats-bootstrap-job.yaml`; `cargo test -p deepref-worker --test delivery_policy --locked` and `mise exec -- just helm-check`.
- **Manual drill/apply-time evidence:** Inject one malformed and one controlled retryable event into deployed development/staging; show maximum five deliveries with `5s,30s,2m,10m,30m` backoff, deterministic `dead_letter_records`/outbox entry, `DEEPREF_DLQ` record, terminated source delivery, and no loop.
- **Prerequisites:** Deployed JetStream/worker, approved test subjects/data, NATS observer access, database diagnostic role, and cleanup/retention plan.
- **Current verification status:** **Partial.** Policy is unit/static tested; no live delivery/DLQ evidence exists.

## AC-06 — NATS quorum

- **Criterion:** Killing one NATS pod preserves quorum and processing.
- **Automated evidence path/command:** `charts/deepref/tests/nats_test.yaml`, staging/production value fixtures, `observability/alerts/nats-worker.yaml`; `mise exec -- just helm-check`.
- **Manual drill/apply-time evidence:** In a deployed three-replica staging cluster, delete/evict one NATS pod under an approved drill; retain leader/replica and consumer reports, successful publish/process continuity, resynchronization, and alerts.
- **Prerequisites:** Deployed EKS, NATS three-replica streams across suitable nodes/AZs, credentials, spare capacity, and [NATS recovery runbook](../operations/runbooks/nats-quorum-dlq-recovery.md).
- **Current verification status:** **Pending apply-time drill.** Local NATS is intentionally single-node and cannot prove quorum.

## AC-07 — Graph-only degradation

- **Criterion:** Stopping Neo4j preserves core operations and produces typed graph-only degradation.
- **Automated evidence path/command:** `crates/http-api/tests/degradation.rs`, `crates/http-api/src/error.rs`, web degraded-state components/tests, chart probes; `cargo test -p deepref-http-api --test degradation --locked` and `mise exec -- just test-e2e`.
- **Manual drill/apply-time evidence:** Stop/fail Neo4j in development/staging through an approved drill; prove core project/article/ingestion/settings workflows remain usable, graph/recommendations return `503 GRAPH_UNAVAILABLE` with `Retry-After`, stale metrics are labeled, UI degradation renders, and recovery succeeds.
- **Prerequisites:** Deployed stack, Cloudflare-authenticated test user/synthetic, representative data, and graph/core monitoring.
- **Current verification status:** **Partial.** Typed response construction and mocked UI paths exist; no running Neo4j outage drill is retained.

## AC-08 — Representative graph rebuild

- **Criterion:** The representative graph rebuild completes in under 60 minutes with count/hash parity.
- **Automated evidence path/command:** `services/projector/tests/rebuild.rs`, ignored `services/projector/tests/rebuild_performance.rs`; `cargo test -p deepref-projector --test rebuild_performance -- --ignored --nocapture`.
- **Manual drill/apply-time evidence:** Rebuild 250,000 works and 2.5 million edges against deployed production-like PostgreSQL/Neo4j; retain eight-stage timing, watermark/replay, work/membership/citation counts, sampled deterministic hashes, final lag/state, and core availability.
- **Prerequisites:** Representative generated dataset, sufficient staging capacity, approved rebuild workflow/GitOps values change, monitoring, and [Neo4j rebuild runbook](../operations/runbooks/neo4j-rebuild.md).
- **Current verification status:** **Pending.** The ignored test currently verifies only the numeric dataset shape; it does not execute a rebuild. The required App-authored values-change workflow/policy is also missing.

## AC-09 — Failed migration blocks rollout

- **Criterion:** A failed migration blocks rollout while the previous application continues serving.
- **Automated evidence path/command:** `charts/deepref/templates/migration-job.yaml`, `charts/deepref/tests/migration_test.yaml`, `crates/http-api/tests/migration.rs`; `mise exec -- just helm-check`.
- **Manual drill/apply-time evidence:** Deploy an intentionally failing additive test migration in a safe environment, show Argo PreSync failure before Deployment change, old ReplicaSet and core endpoints serving, then safe-stop/forward-fix recovery.
- **Prerequisites:** Deployed Argo/chart, immutable test release, reversible safe test schema, monitoring, data owner, and [migration-failure runbook](../operations/runbooks/migration-failure.md).
- **Current verification status:** **Partial.** Hook ordering/backoff are source tested; no live Argo failure drill proves previous service continuity.

## AC-10 — RDS RPO/RTO

- **Criterion:** RDS Multi-AZ failover and PITR drills meet the declared RPO/RTO.
- **Automated evidence path/command:** `infra/modules/rds/**`, `infra/environments/production/main.tf`, module tests; `mise exec -- just infra-validate`.
- **Manual drill/apply-time evidence:** Production or production-equivalent Multi-AZ failover plus isolated PITR restore with approved clock definitions, transaction/data invariants, observed RPO <=5 minutes and RTO <=60 minutes, private posture, and application recovery.
- **Prerequisites:** Applied production RDS, healthy recovery points/quotas, approved validation and [RDS runbook](../operations/runbooks/rds-failover-pitr.md), data/platform owners, and evidence store.
- **Current verification status:** **Pending apply-time drill.** No RDS exists/proof is available from local source, and the reusable AWS Backup module is not wired into environment roots.

## AC-11 — Exact signed digest promotion

- **Criterion:** Exact signed image and chart digests progress through all three environments.
- **Automated evidence path/command:** `.github/workflows/{release,promote-staging,promote-production}.yml`, `scripts/ci/{copy-oci-release,verify-release-digests}.sh`, schemas/fixtures; run schema/fixture validation and workflow policy checks.
- **Manual drill/apply-time evidence:** One real release manifest and three merged locks showing identical source tree/subjects (registry location may change), signatures/referrers/attestations copied and verified, plus pod image IDs/Argo deployment evidence in each environment.
- **Prerequisites:** Three ECR accounts/roles, GitHub OIDC/protected environments/App, protected GitOps branch, Argo, cosign/OCI support, and deployed earlier-environment evidence.
- **Current verification status:** **Partial.** Workflows/scripts exist; there is no GitOps branch, OCI release, ECR copy, or deployed three-environment proof.

## AC-12 — Unsigned/unapproved rejection

- **Criterion:** Unsigned or unapproved images are rejected.
- **Automated evidence path/command:** `charts/deepref/templates/kyverno-verify-images.yaml`, `charts/deepref/tests/image_verification_test.yaml`, `policy/helm/immutable-images.rego`; `mise exec -- just helm-check`.
- **Manual drill/apply-time evidence:** On deployed staging/production admission policy, submit controlled unsigned and wrong-identity digest Pods and retain admission denial; show an approved signed digest succeeds. Do not weaken policy for the test.
- **Prerequisites:** Installed/configured Kyverno/controller, mirrored digest-only images, correct keyless issuer/subject, private EKS access, and safe test namespace/process.
- **Current verification status:** **Partial.** Render/unit/policy source exists; no server-side admission rejection evidence exists.

## AC-13 — Access denial and no origin bypass

- **Criterion:** A nonmember GitHub identity is denied and no public AWS origin path exists.
- **Automated evidence path/command:** `infra/modules/cloudflare-perimeter/**`, `observability/synthetics/src/access-denial-canary.ts`, module/static tests; `tofu -chdir=infra/modules/cloudflare-perimeter test` and `infra/tests/static-contracts.sh`.
- **Manual drill/apply-time evidence:** Cloudflare Access test for member allow and nonmember deny, DNS/tunnel/JWT validation proof, AWS inventory showing no public application load balancer/IP/hostname, and synthetic alert delivery.
- **Prerequisites:** Applied global/per-environment infrastructure, GitHub OAuth/organization, Cloudflare zone/domain/tunnels/tokens, nonmember test identity, and confirmed monitoring.
- **Current verification status:** **Pending apply-time drill.** IaC source exists, but Cloudflare/AWS origin state is not locally provable.

## AC-14 — GitOps rollback

- **Criterion:** Rollback restores the previous compatible release through a GitOps PR.
- **Automated evidence path/command:** `.github/workflows/rollback.yml`, release-lock validators/fixtures; validate a fixture and inspect same-migration guard.
- **Manual drill/apply-time evidence:** Staging then production drill selecting a reviewed prior GitOps commit with the same migration version; retain protected workflow, App PR/approvals, merged lock, Argo sync, restored pod digests, and symptom recovery.
- **Prerequisites:** GitOps history, two compatible releases still in ECR, protected App/environments, Argo, and [rollback runbook](../operations/runbooks/rollback.md).
- **Current verification status:** **Partial.** Protected workflow contract exists; no GitOps branch/deployed rollback evidence exists.

## AC-15 — Empty plan and healthy Argo

- **Criterion:** `tofu plan` is empty after apply and Argo reports every application healthy and synced.
- **Automated evidence path/command:** `.github/workflows/infra-apply.yml` runs post-apply `tofu plan -detailed-exitcode`; local source validation is `mise exec -- just infra-validate`.
- **Manual drill/apply-time evidence:** Successful apply run per root with exit code `0` on follow-up plan, drift review, and exported Argo application list showing every environment application `Synced`/`Healthy` at the accepted GitOps revision.
- **Prerequisites:** Remote backends, applied roots, protected OIDC roles/environments, private EKS runner, GitOps branch, and Argo children.
- **Current verification status:** **Pending apply-time apply.** No infrastructure/Argo deployment evidence exists.

## AC-16 — No supported self-host path

- **Criterion:** Documentation contains no supported self-host deployment path.
- **Automated evidence path/command:** `scripts/check-docs.sh` scans current documentation/obsolete paths and verifies operations structure/internal links; `bash scripts/check-docs.sh`.
- **Manual drill/apply-time evidence:** Documentation review confirms Compose is only loopback disposable local dependencies; README/contribution/security/architecture/operations entry points point to ECR/Helm/Argo/OpenTofu for hosted operation.
- **Prerequisites:** All documentation changes present; protected plan excluded from obsolete-guidance scan because it is historical requirements context and remains unchanged.
- **Current verification status:** **Verified locally.** On 2026-08-05, `bash scripts/check-docs.sh` passed with exactly eight guides, twelve runbooks, all sixteen IDs, valid internal links/alert runbook paths, and no obsolete deployment paths. The legacy deployment example files are already removed in the preserved dirty-tree work; hosted documentation review remains part of final sign-off.

## Acceptance sign-off rule

Production acceptance requires every criterion to be **Accepted**, with evidence location, reviewer, and date recorded by the accountable owner. Local commands can close only locally provable portions. EKS/RDS/NATS/Neo4j/Argo/Cloudflare/GitHub/admission drills remain pending until the three AWS accounts and global prerequisites are deployed.
