# Handoff 07: production object-storage boundary

## Goal

Ensure large documents and artifacts use S3-compatible object storage in production instead of turning PostgreSQL into blob storage.

Read `00_PROGRAM.md` and complete handoff 01 first.

The repository already has an object-store abstraction. Extend and standardize it rather than introducing a second storage layer.

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

## Tests

Cover:

- tenant isolation;
- unauthorized signed-URL request;
- failed upload;
- DB commit failure after upload;
- deletion retry;
- hash mismatch;
- development filesystem backend;
- production S3-compatible backend contract.

## Completion criterion

Production PostgreSQL stores document/artifact metadata and provenance rather than large binary payloads, while object access remains tenant-authorized, integrity-checked and recoverable after partial failures.
