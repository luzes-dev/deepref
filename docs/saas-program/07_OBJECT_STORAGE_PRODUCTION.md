# Handoff 07: production object-storage boundary

## Goal

Ensure large documents and artifacts use S3-compatible object storage in production instead of turning PostgreSQL into blob storage.

Read `00_PROGRAM.md` and complete handoff 01 first. This handoff runs alongside 02 and 03; its production cutover requires the 03 tenancy gate (authorization before object access).

The repository already has an object-store abstraction. Extend and standardize it rather than introducing a second storage layer.

## Blob inventory and database migration

Audit existing PostgreSQL schemas for binary and oversized text payloads:

- `workflow_files.content bytea` up to 16 MiB in `crates/app/migrations/0028_workflow_engine.sql` must move to object storage.
- Grep `crates/app/migrations/` for other `bytea` or large text columns that store raw file bodies.

Migration sequence for `workflow_files`:

```text
1. Expand: add object storage reference columns (storage key, sha256 hash, byte size) to workflow_files while keeping content nullable.
2. Dual-write: write new files to object storage and populate metadata columns; fall back to reading content bytea if storage key is null.
3. Backfill: run an idempotent background task reading content bytea, writing to object storage, verifying sha256 hash against written bytes, and setting storage key.
4. Contract: drop content bytea in a subsequent release once all rows reference verified objects (see 12_OPERATIONS_ACCEPTANCE.md).
```

## Scope

Audit and productionize storage for large or binary objects such as:

```text
PDFs
uploaded documents
parsed artifacts
large exports
large workflow/review artifacts where applicable
```

PostgreSQL should keep:

```text
ownership
workspace/project scope
storage key
content hash
MIME/type metadata
size
provenance
lifecycle status
```

Object storage keeps bytes.

## Provider abstraction

Use the existing `object_store` integration where practical.

Production must be able to target an S3-compatible provider without domain code knowing whether it is:

```text
AWS S3
Cloudflare R2
another compatible provider
```

Development may retain filesystem backing.

## Tenant-safe key layout

Storage keys must not depend on user-provided filenames.

Use stable IDs and/or content hashes.

Example shape:

```text
workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/...
```

Do not rely on path naming as the authorization boundary. Authorization happens before generating or reading object references.

## Integrity

Record content hashes where useful.

After upload, application state should be able to verify that the persisted object corresponds to the expected artifact.

Avoid partially committed records:

- either upload to a temporary key then commit/finalize;
- or persist an explicit pending state and reconcile failures.

Choose the approach that matches current document workflows.

## Access

Prefer server-mediated access or short-lived signed URLs.

Do not make buckets public.

A signed URL must be generated only after DeepRef authorization establishes resource access.

Keep URL expiry short enough for the workflow.

## Deletion and lifecycle

Define behavior for:

```text
project deletion
workspace deletion
replaced document versions
failed imports
temporary uploads
exports
```

Use durable cleanup jobs rather than requiring object deletion inside a database transaction.

Cleanup must be idempotent and use the outbox/worker infrastructure when available.

## Out of scope

Do not migrate small relational JSON fields or normalized scientific data merely to reduce database size.

Do not introduce a separate document microservice.

Assistant SSE (`crates/http-api/src/routes/assistant/run_events.rs`) replays persisted events from PostgreSQL and already spans API replicas, so no pub/sub is needed for event streams.

## Tests

Cover:

- tenant isolation;
- unauthorized signed-URL request;
- workflow_files staged backfill to object storage with sha256 hash verification;
- expand/contract dual-write and fallback read when storage key is null;
- backfill task idempotency and resume after interruption;
- failed upload;
- DB commit failure after upload;
- deletion retry;
- hash mismatch;
- development filesystem backend;
- production S3-compatible backend contract.

## Completion criterion

Production PostgreSQL stores document/artifact metadata and provenance rather than large binary payloads, `workflow_files.content` has migrated safely to object storage via verified backfill and expand/contract, while object access remains tenant-authorized, integrity-checked and recoverable after partial failures.
