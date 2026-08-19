# DeepRef

DeepRef maps article citation networks from seed DOIs. It ships a SvelteKit web
app, a Rust Axum API, a Rust worker, and PostgreSQL state plus graph storage.

## Layout

```text
apps/web        SvelteKit frontend
crates/*        Rust library crates, including the HTTP API and PostgreSQL adapter
services/worker Rust ingestion worker
infra           Local dependencies and production infrastructure
 docs            Architecture, API, operations, and acceptance
```

## Requirements

- mise
- Docker

## Setup

```bash
mise trust
mise install
mise exec -- just bootstrap
```

## Development

```bash
mise exec -- just dev
```

See [Local development](docs/local-development.md) for ports, seed data, reset behavior, and the full command surface. Compose is disposable local dependency tooling only; it is not a deployment artifact.

Production operations are documented in the [operations index](docs/operations/README.md). The source/apply/drill boundary is tracked in [AC-01 through AC-16](docs/acceptance/production-platform.md); repository source does not by itself mean AWS, Cloudflare, Argo, or RDS has been deployed or drilled. See the [documentation index](docs/README.md) for all platform references.

## Checks

```bash
mise exec -- just verify
mise exec -- just test-unit
mise exec -- just test-integration
mise exec -- just test-e2e
mise exec -- just docs-check
```

Run the TypeScript quality audit directly with:

```bash
mise exec -- pnpm run quality:ts
```

## Development Workflow

Daily work happens through pull requests into `development`. Promote tested work with PRs from `development` to `staging`, then from `staging` to `main`. Emergency `hotfix/*` branches may target `main` directly, followed by back-merge PRs into `staging` and `development`.

## Containers

```bash
mise exec -- just docker-build
```

Protected delivery workflows publish immutable images to ECR and promote exact signed digests through Helm and Argo CD without rebuilding them.

Hosted workloads are Argo-owned. Normal release, rollback, rebuild, and configuration changes use reviewed GitOps automation; direct workload mutation is reserved for explicitly authorized break-glass incidents.
