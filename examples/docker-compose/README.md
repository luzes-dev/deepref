# Self-hosting Example

This folder contains a full-stack Compose example for running DeepRef on one host.

## Services

- `web`: static SvelteKit SPA served by Caddy on `http://localhost:3000`
- `api`: Rust Axum API on `http://localhost:8080`
- `worker`: Rust ingestion worker
- `postgres`: transactional application state
- `nats`: event bus with JetStream enabled
- `neo4j`: graph database, with constraints available under `infra/neo4j`

## Run

```bash
cp .env.example .env
docker compose --env-file .env -f docker-compose.selfhost.yml up --build
```

Open:

```text
http://localhost:3000
```

Before running a real ingestion, open `/settings` and set `crossref_mailto`.

If one of the host ports is already in use, set the corresponding `*_HOST_PORT`
variable in `.env`. The service ports used inside the Compose network do not
change. For example, to run alongside another local stack:

```text
POSTGRES_HOST_PORT=5433
NATS_HOST_PORT=4223
NATS_MONITOR_HOST_PORT=8223
NEO4J_HTTP_HOST_PORT=7475
NEO4J_BOLT_HOST_PORT=7688
API_HOST_PORT=8081
WEB_HOST_PORT=3001
```

The web container proxies `/api/*` to the Compose `api:8080` service, so the
browser always uses the web origin and does not need a separate API URL or CORS
configuration.

## Local CORS

The API supports two environment variables for browser testing:

```text
API_CORS_ORIGINS=http://localhost:3000,http://127.0.0.1:3000,http://localhost:5173,http://127.0.0.1:5173
API_CORS_ALLOW_ANY=false
```

If your local browser origin changes frequently, set this in `.env` for local-only testing:

```text
API_CORS_ALLOW_ANY=true
```

Do not use `API_CORS_ALLOW_ANY=true` for a public deployment. For hosted use, set `API_CORS_ORIGINS` to the exact web origin, for example `https://deepref.example.org`.

## Neo4j Constraints

After Neo4j starts, apply graph constraints from:

```text
../../infra/neo4j/constraints.cypher
```

The current API vertical slice stores operational graph projections in PostgreSQL and includes the Neo4j model/queries for the graph layer.

## Release Images

Tagged releases publish GHCR images:

```text
ghcr.io/<owner>/deepref-api
ghcr.io/<owner>/deepref-worker
ghcr.io/<owner>/deepref-web
```
