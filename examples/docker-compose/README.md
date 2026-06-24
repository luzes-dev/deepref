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

## Public API URL

`PUBLIC_API_BASE_URL` is read by the web container at startup. To change it, update the environment and restart/recreate the web container. No image rebuild is needed:

```bash
PUBLIC_API_BASE_URL=https://api.example.org \
docker compose --env-file .env -f docker-compose.selfhost.yml up -d web
```

For a single host using the default port mapping, keep:

```text
PUBLIC_API_BASE_URL=http://localhost:8080
```

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
