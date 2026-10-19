# ADR 0008: Production deployment ownership and target

## Status

Proposed.

## Context

Handoff 12 (operations acceptance) requires establishing production deployment
ownership, an operational target platform, expand/contract migration
sequencing, point-in-time recovery with object-store reconciliation, and
service-level objectives. Previous deployment assets (`charts/deepref`,
`infra/`, `observability/`) described an unchosen target and were removed in
commit `e70e800`.

This ADR records operational parameters, target requirements, release
sequencing, and compatibility verification rules before production launch.
Where business gates or platform selections remain unfinalized, decisions are
explicitly flagged as pending with designated decision-makers and required
inputs. No cloud provider is chosen in this document.

## Decision

### Deployment ownership and target

#### Owner

Decision pending: Program sponsor and engineering leadership decide; requires defining the on-call rota, production migration approval authority, and drill execution assignments.

The owner is the team or role paged for production incidents, who approves
production migrations, and who executes quarterly restore drills.

#### Target

Decision pending: Platform owner decides; requires platform evaluation for hosting container images (`api`, `worker`, `web`), managed PostgreSQL with continuous WAL archiving, and an S3-compatible object storage service.

The target provides the runtime platform for the API, worker, and web images,
the PostgreSQL 17 database with point-in-time recovery (PITR), and the
S3-compatible bucket.

#### Region

Decision pending: Program sponsor and compliance lead decide; depends on the LGPD policy and Polar payout entity from the 00 business gate.

Data placement must satisfy the governing LGPD data residency and privacy
policies.

#### Secrets

Decision pending: Security and platform leads decide; requires selecting secret management infrastructure compatible with the target platform and establishing rotation schedules.

Specifies storage, injection, and rotation ownership for:

- `DOCUMENT_STORAGE_*`
- `WORKOS_*`
- `POLAR_*`
- `OTEL_EXPORTER_OTLP_ENDPOINT`

#### Migrations

Production database migrations run exclusively via `deepref-server migrate`
executed once against the target database before any version N replica starts.
The HTTP API (`serve`) never runs migrations.

CI migration compatibility checks (`migration-compat`, `append-only`, and
`destructive-sql`) run in report-only mode until the repository has its first
`v*` tag. Once release tags exist, these checks become blocking gates.

#### Metrics

Decision pending: Observability lead decides; requires selecting the metrics backend and OpenTelemetry metrics exporter for the SLIs defined in handoff 12.

Currently, `crates/telemetry` exports traces only. Metric instruments must be
added to API and worker services to supply time-series SLIs.

#### Environments

DeepRef operates with two environments only: development and production
(per handoff 00). There is no permanent staging environment. Migration rehearsal
and drill operations run against an isolated copy restored from production.

### Target requirements

The deployment target platform must satisfy all of the following requirements:

- **Stateless replicas and draining:** API and worker replicas must be
  stateless and drain cleanly on SIGTERM within the platform grace period,
  including terminating open assistant Server-Sent Events (SSE) connections.
- **Private database network:** A private network path to PostgreSQL with no
  public database endpoint.
- **No public API port initially:** The API port must not be exposed publicly
  until handoff 02 adds authentication.
- **S3 storage backend:** `DOCUMENT_STORAGE_BACKEND=s3` is required in
  production. The `local` storage backend is refused in production (handoff 01).
- **Fail-closed environment:** The application fails closed if `APP_ENV` is
  unset (handoff 01).

### Release order

Rolling deployments overlap binaries N and N-1 against a single database
schema. Releases must execute in the following strict order:

1. Run `deepref-server migrate` once against the target database before any
   version N replica starts.
2. Roll worker and API replicas to version N. Version N-1 replicas continue
   serving active traffic until drained.
3. Verify: each version N replica's `/health/ready` reports `schema_version`
   equal to `required_schema_version`; `/health/dependencies` reports the
   expected `live_workers` count.
4. Run any post-deploy operator actions documented in the release notes.

**Rollback procedure:** If release verification fails, redeploy version N-1
container images against the version N database schema. Never execute
down-migrations.

### Storage reconciliation command

Object-store and database reconciliation is a read-only command that reports
mismatches and never deletes data. It is implemented in `tools/xtask` or a
documented standalone script. Adding a fifth subcommand to `deepref-server` is
explicitly rejected to preserve the runtime role contract established in ADR 0001.

## Consequences

- Deployments operate safely across rolling replica windows where binaries N and
  N-1 run concurrently.
- No staging environment overhead is incurred; schema rehearsals and drills use
  isolated production restore copies.
- CI migration checks remain informative until production versioning begins with
  the first `v*` tag.
- Final infrastructure provisioning remains blocked until pending ownership,
  target, region, and secret decisions are resolved.
