# DeepRef API

Default API base URL: `http://localhost:8080`.

The authoritative API contract is [`openapi.json`](./openapi.json). The running API serves the
same document from:

```text
GET /openapi.json
```

The API binary has explicit commands:

```bash
deepref-server serve
deepref-server migrate
deepref-server --print-openapi
```

`serve` never applies migrations. Hosted migrations run as the Helm/Argo PreSync Job; local development uses `just migrate`.

Regenerate the Rust-owned OpenAPI document and typed web client with:

```bash
mise exec -- just codegen
```

Verify committed code generation is current with:

```bash
mise exec -- just codegen-check
```

## Local CORS

The API reads CORS settings from:

```text
API_CORS_ORIGINS=http://localhost:3000,http://127.0.0.1:3000
API_CORS_ALLOW_ANY=false
```

Use `API_CORS_ALLOW_ANY=true` only for local testing when the browser origin is changing.

## Health and degradation

The v2 review workflow exposes:

- GET /projects/{project_id}/protocol returns the published review protocol.
- GET /projects/{project_id}/screening/title-abstract returns the bounded v2 screening queue.
- POST /projects/{project_id}/reports/{report_id}/screening appends a screening decision. The request includes
  protocol_version_id and expected_revision; stale writes return 409.
- GET /projects/{project_id}/prisma returns the live canonical PRISMA projection.
- GET /projects/{project_id}/graph?fields=... returns bounded citation nodes with compact, explicitly selected overlays.
- GET /projects/{project_id}/exports/{kind} returns a deterministic report, PRISMA, audit, or protocol export.

## Notifications

Async server-side outcomes (import/acquisition completion and failure, review-run completion,
blocking, and failure, automation run completion and failure) are recorded as user-facing
notifications in the same transaction that made the underlying transition authoritative. The
inbox is bounded by retention pruning (read rows older than 30 days, hard cap on total rows).

- `GET /notifications` lists notifications newest first with bounded cursor pagination.
- `GET /notifications/unread-count` returns the unread count and the newest revision.
- `POST /notifications/mark-read` marks notifications read by ids or all.

The web client polls the unread count and list, renders the notification center from the bell
button in the top navigation bar, and surfaces new notifications as toasts.

- `GET /health/live` reports process liveness.
- `GET /health/ready` checks PostgreSQL reachability and schema compatibility.
- `GET /health/dependencies` reports PostgreSQL and durable worker-job state without making graph queries a separate dependency.
- `GET /projects/{project_id}/projection` exposes project projection state/revision/lag.
- Graph and recommendation routes read the PostgreSQL graph directly and do not return an external-graph `503`.

List responses use bounded cursor pagination (`items` and `next_cursor`). Article/graph responses expose metric freshness/projection metadata as defined by OpenAPI.

Endpoint paths, request bodies, response schemas, operation IDs, and status codes are defined only
in the OpenAPI document to avoid duplicating the Rust contract here.

Cancelling ingestion requires an existing queued/running run. Missing runs return
404; terminal runs (including previously cancelled runs) return 409 without
altering their status or completion timestamp. Cancellation locks the run row
before checking state so concurrent completion cannot be overwritten.

Citation expansion depth is a domain value from zero through three. Project
creation/update, global settings, ingestion, and acquisition reject larger or
negative values with 400. The corresponding database columns enforce the same
range; migration fails on unsafe historical values rather than rewriting them.

Document blocks now use `items`/`next_cursor` pages ordered by page number,
ordinal and block UUID (maximum 100 blocks per request). Full-text screening,
appraisal, extraction and citation labels collect every page before exposing
overlay/evidence blocks; a later-page failure is an error, not a partial paper.

The workspace keeps explicitly selected project URLs on load failures. Transient
errors show “Project unavailable” with Retry; 404 shows “Project not found” with
Retry. Automatic default-project selection applies only to routes with no project
identifier, never to a failed named project.

AI proposal decisions invalidate generated title/abstract queue, full-text queue,
proposal and deduplication keys scoped to the mutation's project. Suggestion
creation invalidates only that project's proposal list. Generated clients must
not emit a bare `/api/projects/` invalidation prefix.

Workflow HTTP credentials are stored with their entered URL origin. Changing
origin drops the credential, while changing only path/query retains it. Execution
also matches origin, preventing an older published graph from borrowing a
credential later entered for a different host. Legacy unbound HTTP credentials
require re-entry. HTTP templates require a literal scheme/authority; substituted
path/query values are percent-encoded as components.
