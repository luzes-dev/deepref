# DeepRef architecture

DeepRef maps article citation networks from seed DOIs. The SvelteKit SPA uses a same-origin `/api` contract through the web gateway. A Rust Axum API owns HTTP behavior; a Rust worker performs durable recursive ingestion; a Rust projector maintains the graph read model.

## Data and event planes

PostgreSQL 17 is authoritative for projects, ingestions/items, works, project membership, canonical fetched-reference facts, citations, unresolved references, claims/leases, outbox rows, domain events, projection state, and metric snapshots.

The worker claims versioned work events and DOI leases transactionally, performs the provider request outside the final transaction while renewing the lease, then atomically attaches cached/fetched facts, inserts discovered children, emits deterministic outbox/domain events, completes the item/claim, and commits before ACK. PostgreSQL also owns the provider-wide permit schedule and reconciliation state.

Hosted NATS JetStream resources are pre-provisioned by the Helm/GitOps bootstrap Job; application credentials do not administer them:

| Stream           | Subjects/purpose              | Hosted contract         |
| ---------------- | ----------------------------- | ----------------------- |
| `DEEPREF_WORK`   | `work.fetch.requested.v1`     | Work-queue retention    |
| `DEEPREF_DOMAIN` | `domain.>` and `projection.>` | 30-day limits retention |
| `DEEPREF_DLQ`    | `dlq.recorded.v1`             | 90-day limits retention |

Staging/production use three stream replicas; development/local use one. Durable consumers are `deepref-worker` and `deepref-projector`, with five bounded deliveries and backoff `5s, 30s, 2m, 10m, 30m`.

Neo4j Community is a single-node, asynchronous projection used for graph/recommendation reads and graph metrics. The projector applies versioned constraints/indexes, processes V1 domain events idempotently with entity revision cursors, and updates projection status/metric freshness. PostgreSQL can rebuild Neo4j through the explicit `deepref-projector rebuild --run-id <UUID>` flow. During Neo4j failure, core API readiness remains PostgreSQL-based while graph routes return typed `503 GRAPH_UNAVAILABLE` with `Retry-After`.

## API and web degradation

The API exposes process liveness, PostgreSQL/schema readiness, dependency detail, and per-project projection status. Collections use bounded cursor pagination. The web app polls dependency status independently and keeps project, article metadata, ingestion, settings, and navigation available during graph degradation.

The application has no user/tenant model. Cloudflare Access protects each hosted environment using configured GitHub organization membership, and all admitted users have equal application privileges.

## Hosted platform ownership

- OpenTofu roots under `infra/environments` own AWS infrastructure and global Cloudflare/GitHub policy plus initial Argo installations.
- The protected orphan `gitops` branch and Argo own hosted namespaces, workloads, NATS, Neo4j, External Secrets, policies, telemetry collectors, and `cloudflared`.
- Releases build four images and one chart once from tested development source, sign/attest them, then copy exact OCI subjects through staging and production without rebuild.
- PostgreSQL migrations run only through `deepref-api migrate`; hosted Helm runs it as an Argo PreSync Job before Deployment changes. Normal `deepref-api serve` never migrates.

The repository contains deployable source, not proof of a deployed platform. Current apply/drill gaps are recorded in [production acceptance](acceptance/production-platform.md) and [operations](operations/README.md).

## Local development

```bash
mise exec -- just dev
```

This starts loopback-only PostgreSQL, single-node NATS JetStream, and Neo4j as disposable dependencies, applies migrations/JetStream bootstrap, then supervises web, API, worker, and projector. Compose contains no application services and is not a deployment artifact. See [local development](local-development.md).
