# DeepRef Architecture

DeepRef maps article citation networks from seed DOIs. The system uses SvelteKit for the internal UI, a Rust Axum API for HTTP and realtime endpoints, a Rust worker for recursive ingestion, PostgreSQL for transactional state, Neo4j for the property graph model, and NATS JetStream-compatible subjects for event-driven ingestion.

The current implementation stores the operational graph projection in PostgreSQL for the API vertical slice and includes Neo4j constraints and Cypher query constants for the planned graph database layer. The worker is queue-driven and never recurses in-process: discovered references publish new `work.fetch.requested` events with incremented depth.

Crossref access is handled by `deepref-crossref`. Requests use:

- `GET /works/{doi}?mailto={configured_email}`
- `Accept: application/json`
- `User-Agent: deepref/0.1 (mailto:{configured_email})`

Deduplication is enforced by:

- `ingestion_items(ingestion_id, canonical_doi)`
- `doi_fetch_state(canonical_doi)`
- `works(canonical_doi)`
- `citations(project_id, source_doi, target_doi)`

Run local infrastructure:

```bash
docker compose -f infra/docker-compose.yml up -d
```

Run API:

```bash
DATABASE_URL=postgres://deepref:deepref@localhost:5432/deepref \
NATS_URL=nats://localhost:4222 \
cargo run -p deepref-api
```

Run worker:

```bash
DATABASE_URL=postgres://deepref:deepref@localhost:5432/deepref \
NATS_URL=nats://localhost:4222 \
cargo run -p deepref-worker
```
