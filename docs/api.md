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

## Bind address and Host allow-list

- `API_BIND_ADDR` defaults to `127.0.0.1:8080`. A non-loopback address refuses to start unless
  `DEEPREF_ALLOW_NETWORK_BIND=1` is set. Set it only when an authenticating proxy is the sole route
  to the API; the API container image sets it so that its proxy can reach it.
- The API answers `403` to any request whose `Host` is not `localhost`, `127.0.0.1`, `[::1]`, the
  bound IP, or the host of an `API_CORS_ORIGINS` entry. This blocks DNS rebinding. Ports are not
  compared. `/health/live` and `/health/ready` skip the check for orchestrator probes.
- The Vite dev proxy rewrites `Host` to its target (`changeOrigin`), so `API_PROXY_TARGET` needs no
  extra configuration. A reverse proxy that forwards the browser's `Host` (the bundled Caddy does)
  needs its public origin in `API_CORS_ORIGINS`.

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
