# Handoff 12: operations acceptance

## Goal

Make the launch system operable: a recorded deployment owner and target, expand/contract migrations that survive rolling deploys, restore drills that prove PostgreSQL and object storage agree, SLOs with alerts, dependency-failure tests, and a CI service for every new infrastructure dependency.

This handoff is launch-required. It starts with 01 (deployment ownership, CI services) and closes after the rest of the launch set: 02, 03a–c, 04a, 05a, 07, 08, 09, and 11 Parts B and C.

Read `00_PROGRAM.md` first.

## Scope

Implement or record:

- a deployment ownership and target decision, recorded as an ADR;
- a release order that tolerates binaries N and N-1 against one schema;
- migration checks in CI;
- PostgreSQL backups with point-in-time recovery, object-storage retention, and a restore drill with reconciliation;
- SLOs, alerts and the metrics pipeline they need;
- dependency-failure tests;
- CI services for new infrastructure;
- runbooks and their index.

## Repository baseline

Verify each item before editing. Line numbers drift.

```text
roles        apps/server/src/command.rs: serve | worker | all | migrate | --print-openapi.
             No arguments runs all. serve never migrates (docs/adr/0001-single-server-multiple-roles.md).
images       docker-bake.hcl: api and worker from docker/rust-service.Dockerfile (one binary,
             CMD selects the role); web from apps/web/Dockerfile (Caddy).
publish      .github/workflows/publish-images.yml: v* tags; requires a successful ci.yml run
             for the same commit; pushes to ghcr.io.
deploy       No deploy manifests in the tree. Commit e70e800 removed charts/deepref (Helm, ADOT),
             infra/ (OpenTofu, AWS sa-east-1), docs/operations/, observability/ and policy/helm/.
             docs/api.md:20 still names a Helm/Argo PreSync migration job.
local        compose.yaml (PostgreSQL 17 with pgvector); process-compose.yaml (api, worker, web).
             process-compose.yaml and justfile still default APP_ENV to local. 01 removes that.
migrations   crates/app/migrations, latest 0082. Applied files are immutable
             (crates/app/migrations/README.md). 0080 and 0081 were an offline, stop-the-world cutover.
             This checkout has no git tags.
readiness    /health/ready returns 503 when the database schema is older than the binary's newest
             migration. It only warns when the schema is newer (crates/http-api/src/routes/health.rs).
startup      serve (when an AI key is set, crates/http-api/src/lib.rs) and worker startup
             (services/worker/src/lib.rs) both call ensure_default_model_routes, which can repoint routes.
telemetry    crates/telemetry exports OTLP traces only. No metric instruments exist in crates/,
             services/ or apps/. /health/dependencies computes backlog, oldest queued age and live
             workers with SQL, so there is no time series to alert on.
ci           ci.yml full-stack starts PostgreSQL with docker run and applies migrations with
             cargo sqlx migrate run. No NATS, Valkey or object-store service exists in CI.
docs         scripts/check-docs.sh fails on broken relative links under docs/. docs/README.md
             indexes four documents and neither runbook.
security     docker/rust-service.Dockerfile: the API has no authentication. Do not publish its port
             until 02 lands.
```

## Deployment ownership and target

This is a decision to record, not a code task. Record it in a new ADR under `docs/adr/` before 08 starts. The region depends on the LGPD policy and the Polar payout entity from the 00 business gate. This handoff does not choose a provider.

The ADR records:

```text
owner        the team or role paged for production, that approves production migrations,
             and that runs restore drills
target       platform for the api, worker and web images, the PostgreSQL service with PITR,
             and the S3-compatible bucket
region       data placement under the LGPD policy
secrets      where DOCUMENT_STORAGE_*, WORKOS_*, POLAR_* and OTEL_EXPORTER_OTLP_ENDPOINT live,
             and who rotates them
migrations   how `deepref-server migrate` runs once per release (see below)
metrics      the backend and exporter for the SLIs in this handoff
environments development and production only (00). Migration rehearsal uses a restored
             production copy, not a staging environment.
```

The target must also provide:

- stateless API and worker replicas that drain on SIGTERM within the platform grace period, including ending assistant SSE connections;
- a private network path to PostgreSQL, with no public database endpoint;
- no public API port until 02 adds authentication;
- `DOCUMENT_STORAGE_BACKEND=s3` in production. The `local` backend is refused there (01);
- fail-closed behavior when `APP_ENV` is unset (01).

## Migrations and rolling deploys

Binaries N and N-1 run against the same schema during a rolling deploy. Every release must work for that overlap window in both directions.

Release order:

```text
1  deepref-server migrate, run once against the target database, before any N replica starts
2  roll worker and API replicas to N; N-1 replicas keep serving until drained
3  verify: each N replica's /health/ready reports schema_version equal to required_schema_version;
   /health/dependencies reports the expected live_workers
4  run the post-deploy operator actions listed in the release notes
rollback   redeploy N-1 images against the N schema. Never run down-migrations.
```

Expand/contract phases:

```text
expand     add nullable columns, new tables, new enum values, new indexes
backfill   batched and idempotent, outside the request path
dual       writers write the old and new shapes; readers still read the old shape
switch     readers move to the new shape in a later release
contract   drop, rename, add NOT NULL, narrow a type or remove an enum value. Only after no
           deployed image reads the old shape, including scheduled jobs and worker pools
```

Rules:

- A migration that changes a job kind's payload or state set is contract-sensitive. An N-1 worker must ignore or park the new state safely. Test it.
- Index builds on large tables need a non-transactional migration (sqlx `-- no-transaction`). No migration does this today.
- Startup writes must give the same result under N and N-1. `ensure_default_model_routes` must either be idempotent across both settings or move out of `serve` and `worker` into an explicit step. `docs/local-development.md` already requires API and worker to deploy together for the metadata chain. Extend that rule to every config-dependent startup write.
- Each migration header declares its phase: `-- phase: expand` or `-- phase: contract`. A contract migration names the release that must already be out of service.
- The stop-the-world path used by 0080 and 0081 is allowed only as a declared maintenance window with a stop order in the runbook. It is not the default.

CI check (idea; enforcing once the first release tag exists):

```text
migration-compat
  1. build the schema from the previous release tag's crates/app/migrations on a disposable PostgreSQL
  2. seed representative rows: projects, workspaces, jobs in each state, documents, webhook inbox rows
  3. apply the PR's migrations: this is the prepared N-schema database
  4. make it the previous release's template (below)
  5. assert the template's schema version is the PR's newest migration (the query /health/ready runs)
  6. run the previous release's integration suite (checked out at the tag) against the clones
append-only
  git diff --diff-filter=MD <previous-tag> -- crates/app/migrations must be empty
destructive-sql
  fail on DROP, RENAME, SET NOT NULL, TYPE changes and enum removals, unless the file carries
  `-- phase: contract` and an allowlisted release precondition
```

Until the first `v*` tag exists, run these checks in report-only mode and record that decision in the ADR.

The previous suite cannot read the prepared database through its URL. `isolated_pool_from_env` (`crates/app/src/test_isolation.rs`) keeps only the server and credentials from `DATABASE_URL` and clones `dtpl_<hash>`, a template built from the migrations embedded in that checkout. The harness therefore supplies the template itself: it drops any existing template under the name `template_name` derives from the previous tag's migration set, then clones the prepared database into that name. `ensure_template` finds it and skips the build, so every test clone carries the N schema. Run the harness on a disposable server, because the template name is shared by every checkout with the same migration set. Seed rows reach every test clone too; a previous-suite failure they cause is a compatibility finding to triage, not a reason to remove the seed.

## Backups and recovery

PostgreSQL:

- Base backups plus continuous WAL archiving for point-in-time recovery. Encrypted at rest, private network only, deletion-protected in production.
- Proposed starting values, which the owner confirms before launch: 35-day retention, RPO no more than 5 minutes, RTO no more than 60 minutes. These come from the removed `docs/operations/backup-restore-disaster-recovery.md` in `e70e800^`, not from this tree.
- Do not claim an RPO or RTO until a drill meets it. Record the measured values.
- Backups are personal data. The LGPD retention policy from the 00 business gate sets how long a deleted user's data may remain in a backup.

Object storage:

- Bucket versioning on. Noncurrent-version retention is at least the PostgreSQL PITR window plus a margin. A restored database must never reference a version the bucket has already expired.
- Deletions go through the 07 cleanup jobs. Bucket lifecycle rules alone do not delete application data.
- Objects a restore must resolve: `documents.object_key` (`crates/app/migrations/0006_evidence_workspace.sql`), and `workflow_files.content` until 07 moves it out of `bytea` (`crates/app/migrations/0028_workflow_engine.sql`).

Restore drill:

- Cadence (proposed, owner confirms): before launch, at least quarterly, and before any contract migration.
- Steps:

```text
1  capture the source's latest restorable time and the object-store inventory time
2  restore PITR to a new isolated instance at an approved timestamp. Never overwrite the source first
3  resolve object versions as of the same timestamp into a quarantined bucket or prefix, read-only
4  run the reconciliation below with a read-only database role
5  before any worker starts: point the restored copy at isolated provider credentials and endpoints
   (WorkOS, Polar, AI providers, every other outbound target) and at the quarantined bucket or prefix
   only; block production egress from the drill network; quarantine the restored `queued` and `running`
   jobs after step 4 has read them, so no candidate worker can claim production work. Then start a
   candidate API and worker, enqueue controlled fixture jobs, and verify /health/ready and that the
   fixtures drain. Production jobs never drain in the drill
6  record achieved RPO and RTO, mismatch counts and validation output; then delete the restored instance
```

- Reconciliation reports and never deletes:
  - metadata rows whose object or version is missing;
  - objects with no metadata row (orphans, from the bucket inventory);
  - stored content hashes that do not match recomputed hashes on a sample (07 integrity);
  - jobs `running` with expired leases that workers cannot reclaim, and inbox rows received but never processed;
  - reservations pending settlement past their expiry (09).
- Keep reconciliation as one read-only command. Record its home in the ADR: `tools/xtask` or a documented script. A fifth `deepref-server` subcommand changes ADR 0001 and needs its own ADR.
- Lease fencing inside the restored database fences only the workers that share it. It cannot fence workers on the live database, so the drill's isolation comes from credentials, endpoints and network, never from fencing. The drill network has no route to the live database.

## SLOs and alerts

Owners set every target before launch. No row ships with `TBD`.

| SLI                                                                            | Source today                                                                | Target            | Window | Owner          |
| ------------------------------------------------------------------------------ | --------------------------------------------------------------------------- | ----------------- | ------ | -------------- |
| API availability (non-5xx ratio)                                               | none; needs a metric                                                        | TBD               | TBD    | API owner      |
| API latency p95 by route class, excluding SSE streams                          | none; needs a metric                                                        | TBD               | TBD    | API owner      |
| API replicas below the required schema version                                 | `/health/ready`                                                             | 0 after rollout   | TBD    | API owner      |
| Job queue age per workload pool                                                | `worker.oldest_age_seconds`; workspace or one project; per pool after 05a   | TBD               | TBD    | Worker owner   |
| Dead jobs                                                                      | `worker.recent_failed` (SQL `recent_dead`: dead jobs in a 30-minute window) | TBD               | TBD    | Worker owner   |
| Live workers against expected                                                  | `worker.live_workers` (heartbeat every 15 s, 60 s window)                   | at least expected | TBD    | Worker owner   |
| Outbox backlog age (04a and `ai_autonomy_outbox`)                              | SQL; no metric                                                              | TBD               | TBD    | Platform owner |
| Inbox backlog age and webhook processing lag (WorkOS, Polar)                   | SQL after 04a                                                               | TBD               | TBD    | Billing owner  |
| Usage settlement backlog (expired unsettled reservations, Polar usage backlog) | 09 observability; SQL                                                       | TBD               | TBD    | AI owner       |
| Backup freshness and WAL archive lag                                           | none                                                                        | TBD               | TBD    | Platform owner |
| Object-store error rate and reconciliation orphan count                        | none                                                                        | TBD               | TBD    | Platform owner |

Rules:

- Each SLO has a page threshold and a ticket threshold.
- Each alert names a runbook section and an owner. Its notification route is tested in production with a test alert. An alert without a tested route is not evidence.
- Metric labels carry no workspace, project, user or cost identifiers (09 observability).
- The metrics pipeline is a prerequisite. `crates/telemetry` exports traces only, and no metric instruments exist. This handoff adds the first API and worker metrics. The ADR records the backend.

## Dependency failure tests

Each failure must leave authoritative state uncorrupted. Degrade the feature instead.

```text
PostgreSQL failover  readiness fails during the switch and recovers after it. In-flight jobs are
                     fenced by lease expiry and reach one terminal state. Committed outbox and inbox
                     rows survive. Replay has no duplicate consequential effect.
                     Extend crates/app/tests/jobs.rs (expired leases are fenced).
Object store down    an upload fails without committing a metadata row. A download returns a typed
                     error, not a hang. Deletion jobs retry. Unrelated endpoints keep working.
                     Object-store health is reported in /health/dependencies, not in readiness.
WorkOS outage        valid sessions keep working as 02 allows. A new login returns "provider
                     unavailable", distinct from unauthenticated. The webhook inbox grows and loses
                     nothing. Reconciliation runs after recovery.
Polar outage         locally known entitlements stay usable (08 failure model). Checkout fails
                     cleanly. The usage export backlog grows with immutable identities. No AI call
                     waits on Polar.
```

Conditional rows, not launch-blocking. Add each one only when its trigger fires:

```text
NATS outage    T-NATS. The outbox backlog grows and the relay retries. No committed work is lost.
               No consumer applies a message twice.
Valkey outage  T-VALKEY. Admission falls back to PostgreSQL leased slots with conservative limits.
               No authoritative state changes.
```

Fault injection uses a TCP proxy in front of PostgreSQL and the object store in CI. Choose the proxy in this handoff. A real failover drill runs against a restored copy, never against production.

## CI services

- PostgreSQL 17 with pgvector: the existing full-stack service in `.github/workflows/ci.yml`. Pin by digest, as the Dockerfiles pin their base images. Add a replica only if the failover test needs one.
- S3-compatible object store: new. Required by the 07 production backend contract test. Pin by version and digest, and wait for health before tests run.
- Fault-injection proxy: new. Used by the dependency tests above.
- NATS JetStream and Valkey: added only with `T-NATS` and `T-VALKEY`.
- WorkOS and Polar: recorded fixtures on every PR. Sandbox contract tests run on a schedule with protected secrets and report through the existing `.github/workflows/scheduled-failure.yml`. PR CI never receives provider secrets.

Start each new service with the same shape as the existing PostgreSQL step in `ci.yml`: a pinned image, a health wait, and failure if the wait times out.

## Runbooks

- Location: `docs/`, named `*-runbook.md`, as `docs/assistant-durability-runbook.md` already is.
- Index every runbook from `docs/README.md`, which lists neither runbook today.
- Write each runbook as symptom, then query, then action, then evidence. Do not hard-code `file.rs:line` references. The 2026-10-10 review found the existing runbooks rot that way (`docs/reviews/2026-10-10-evidence/6-testing-ops-dx.md`).
- Required set (proposed names): `release-and-migration-runbook.md`, `database-restore-runbook.md`, `object-store-reconciliation-runbook.md`, `slo-incident-runbook.md`, `provider-outage-runbook.md`.
- The release runbook lists the known post-deploy operator steps. `docs/deployment-recalibration.md` states that no HTTP endpoint or CLI records calibration bundles, so automation admission stays refused after 0056 until someone writes rows by hand. Decide that path before the release that depends on it ships.
- Correct stale references: `docs/api.md:20` (Helm/Argo PreSync) and the `docs/README.md` index.

## Out of scope

- Choosing a provider or region in code. The ADR records the decision.
- Multi-region failover. The target is single-region PITR unless the ADR says otherwise.
- NATS and Valkey operations before their triggers fire.
- Restoring `infra/`, `charts/` or `observability/` from `e70e800^`. They describe a target no one has chosen; treat them as reference only.
- Rewriting historical review documents. Mark them non-normative instead.
- Capacity planning and load testing.

## Tests

Cover at minimum:

- `migration-compat`: the previous release's suite passes against the N schema, seeded with representative rows;
- `migration-compat` harness: a prepared database whose newest migration is not the PR's fails step 5 before the previous suite starts;
- restore drill guard: a candidate worker started on a restored copy claims no quarantined job and reaches no production endpoint (egress fixture);
- every `worker.<field>` named in the SLO table exists on `DependencyDetail` in `crates/http-api/src/routes/health.rs`;
- append-only and destructive-SQL checks, each with a failing fixture;
- readiness: an N binary against an N-1 schema returns 503; an N-1 binary against an N schema stays ready and warns;
- startup writes: `ensure_default_model_routes` under N and N-1 settings does not repoint routes (extend `crates/app/tests/ai.rs`);
- a job state or payload change: the N-1 worker ignores or parks the new state;
- reconciliation fixture with one case per mismatch class: it reports every case and deletes nothing;
- object-version retention is at least the PITR window (checked by the drill);
- metric definitions contain no workspace, project, user or cost label (static test);
- every alert in the SLO table has a runbook heading and a delivery record for its test alert;
- each row of the dependency-failure matrix, with its listed assertions;
- `scripts/check-docs.sh` passes with the new runbooks and the updated index;
- the object-store service container is healthy before the 07 backend contract test starts.

## Completion criterion

Launch operations are accepted when: the ADR names the owner, target, region, secrets, migration runner and metrics backend; `migration-compat` and the append-only check pass against the first release tag; a restore drill on a restored copy, run with isolated credentials and blocked production egress, records RPO and RTO that meet the signed targets, with zero unexplained reconciliation mismatches; every SLO row has a source, a page and a ticket threshold, and a runbook; the dependency matrix passes for PostgreSQL, object storage, WorkOS and Polar; every new infrastructure dependency used by the launch set has a CI service; and every runbook is indexed from `docs/README.md`. The NATS and Valkey rows activate only when their triggers fire.
