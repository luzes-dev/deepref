# DeepRef API

Default API base URL: `http://localhost:8080`.

The authoritative API contract is [`openapi.json`](./openapi.json). The running API serves the
same document from:

```text
GET /openapi.json
```

Regenerate the Rust-owned OpenAPI document and typed web client with:

```bash
pnpm generate:api
```

Verify committed code generation is current with:

```bash
pnpm generate:api:check
```

## Local CORS

The API reads CORS settings from:

```text
API_CORS_ORIGINS=http://localhost:3000,http://127.0.0.1:3000
API_CORS_ALLOW_ANY=false
```

Use `API_CORS_ALLOW_ANY=true` only for local testing when the browser origin is changing.

Endpoint paths, request bodies, response schemas, operation IDs, and status codes are defined only
in the OpenAPI document to avoid duplicating the Rust contract here.
