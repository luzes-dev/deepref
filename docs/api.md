# DeepRef API

Default API base URL: `http://localhost:8080`.

The authoritative API contract is [`openapi.json`](./openapi.json). The running API serves the
same document from:

```text
GET /openapi.json
```

The API binary has explicit commands:

```bash
deepref-api serve
deepref-api migrate
deepref-api --print-openapi
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

- `GET /health/live` reports process liveness.
- `GET /health/ready` checks PostgreSQL reachability and schema compatibility.
- `GET /health/dependencies` reports PostgreSQL, NATS, outbox, worker, Neo4j, and projection state without making graph dependencies part of core readiness.
- `GET /projects/{project_id}/projection` exposes project projection state/revision/lag.
- Graph/recommendation dependency failure returns `503` with `ApiErrorBody.code = GRAPH_UNAVAILABLE` and `Retry-After`; core project/ingestion routes remain available.

List responses use bounded cursor pagination (`items` and `next_cursor`). Article/graph responses expose metric freshness/projection metadata as defined by OpenAPI.

Endpoint paths, request bodies, response schemas, operation IDs, and status codes are defined only
in the OpenAPI document to avoid duplicating the Rust contract here.
