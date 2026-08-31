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

- `GET /health/live` reports process liveness.
- `GET /health/ready` checks PostgreSQL reachability and schema compatibility.
- `GET /health/dependencies` reports PostgreSQL and durable worker-job state without making graph queries a separate dependency.
- `GET /projects/{project_id}/projection` exposes project projection state/revision/lag.
- Graph and recommendation routes read the PostgreSQL graph directly and do not return an external-graph `503`.

List responses use bounded cursor pagination (`items` and `next_cursor`). Article/graph responses expose metric freshness/projection metadata as defined by OpenAPI.

Endpoint paths, request bodies, response schemas, operation IDs, and status codes are defined only
in the OpenAPI document to avoid duplicating the Rust contract here.
