# DeepRef architecture

DeepRef maps article citation networks from seed DOIs. The SvelteKit SPA uses a same-origin `/api` contract through the web gateway. A Rust Axum API owns HTTP behavior; the Rust worker performs durable recursive ingestion and reconciliation; PostgreSQL is authoritative for state, jobs, graph facts, and metrics.

## Data and event planes

PostgreSQL owns projects, ingestions/items, works, project membership, canonical fetched-reference facts, citations, unresolved references, claims/leases, domain events, durable jobs, graph status, and metric snapshots. The v2 evidence workspace is layered alongside the citation-ingestion model: records, reports, studies, screening events/state, review events, documents, audit rows, and leased jobs are all PostgreSQL tables.

Authoritative state changes and their follow-up jobs are committed in one transaction. The worker claims jobs with `FOR UPDATE SKIP LOCKED`, owns a lease with expiry renewal, retries failures with persisted attempts, marks terminal failures dead, and reclaims expired work. Stable dedupe keys prevent duplicate logical jobs. Ingestion preserves cached/fetched facts, discovered children, citations, events, and completion in the same transaction boundary; reconciliation repairs already-committed rows without silently dropping work.

Graph reads use canonical UUIDs from `reports`, `project_reports`, and `citations`. Reports without DOI identifiers remain graph nodes. Responses are deterministic and bounded; edges are UUID-to-UUID and never depend on DOI mapping. Project metrics use the same degree/rank semantics as the legacy fixture and are recomputed after imports and later ingestions. The PostgreSQL graph status endpoint reports metric/projection freshness for compatibility with existing clients; it is not an external graph service.

The v2 screening command writes its event, state projection, and project lifecycle status in one transaction. PRISMA is a live deterministic projection of canonical relational state; no recomputation job or snapshot is authoritative. The command uses optimistic revisions and returns `409 screening_revision_conflict` when another browser, worker, or automation changes the same report.

## Runtime roles and shutdown

`deepref-server serve` runs HTTP only. `deepref-server worker` runs the bounded PostgreSQL job executor. `deepref-server all` runs both roles in one process with coordinated shutdown. Hosted deployments package the same `deepref-server` binary for API and worker targets; local Process Compose supervises the API, worker, and web processes.

Crate boundaries follow dependency direction: `deepref-domain` owns pure invariants, `deepref-application` owns use-case commands and the minimal `JobQueue` port, `deepref-postgres` owns SQLx migrations and adapters, and `deepref-http-api` owns HTTP adapters and SQL-backed handlers. See [ADR 0002](adr/0002-layered-crate-boundaries.md).

## API and web degradation

The API exposes process liveness, PostgreSQL/schema readiness, durable worker-job status, and per-project graph metric freshness. Collections use bounded cursor pagination. The web app polls dependency status independently and keeps project, article metadata, ingestion, settings, and navigation available while queued work drains. Graph and recommendation routes read PostgreSQL directly and do not return an external-graph `503`.

The application has no user/tenant model. Cloudflare Access protects each hosted environment using configured GitHub organization membership, and all admitted users have equal application privileges.

## Hosted platform ownership

- OpenTofu roots under `infra/environments` own AWS infrastructure and global Cloudflare/GitHub policy plus initial Argo installations.
- The protected orphan `gitops` branch and Argo own hosted namespaces, workloads, External Secrets, policies, telemetry collectors, and `cloudflared`.
- Releases build three application images and one chart once from tested development source, sign/attest them, then copy exact OCI subjects through staging and production without rebuild.
- PostgreSQL migrations run only through `deepref-server migrate`; hosted Helm runs it as an Argo PreSync Job before Deployment changes. Normal `deepref-server serve` never migrates.

The repository contains deployable source, not proof of a deployed platform. Current apply/drill gaps are recorded in [production acceptance](acceptance/production-platform.md) and [operations](operations/README.md).

## Local development

```bash
mise exec -- just dev
```

This starts loopback-only PostgreSQL, applies migrations, then supervises web, API, and worker. Compose contains no application services and is not a deployment artifact. See [local development](local-development.md).
