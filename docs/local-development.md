# Local development

DeepRef uses mise for pinned tools, Just for the developer command surface, Docker Compose for disposable dependencies, and Process Compose for application processes.

The Compose file is disposable local tooling only. It contains PostgreSQL and no application services; it is not a deployment artifact or a supported deployment path. Production workloads are built and promoted through the ECR, Helm, Argo CD, and OpenTofu workflow.

## Prerequisites

Install:

- mise
- a Docker-compatible engine with the `docker compose` plugin

From the repository root:

```bash
mise trust
mise install
mise exec -- just bootstrap
```

`bootstrap` creates `.env` from `.env.example` only when `.env` is absent, installs locked JavaScript and Rust dependencies, and installs the Playwright Chromium build.

## Run the stack

```bash
mise exec -- just dev
```

`just dev` waits for PostgreSQL to become healthy, applies PostgreSQL migrations, and then starts the web app, API, and worker under Process Compose. Process names prefix their logs. API readiness gates the other processes, and Ctrl-C sends each service its configured clean shutdown signal in reverse dependency order.

Local endpoints:

| Service         | Address                 |
| --------------- | ----------------------- |
| Web             | `http://127.0.0.1:5173` |
| API             | `http://127.0.0.1:8080` |
| PostgreSQL      | `127.0.0.1:5432`        |

Every published dependency port binds only to `127.0.0.1`.

## Seed data

In another terminal, run:

```bash
mise exec -- just seed
```

The seed is deterministic and idempotent. It creates one project with three works and three citation edges. Its Crossref address is a reserved placeholder; set a real contact address in application settings before starting an actual ingestion.

## Stop or reset

```bash
mise exec -- just dev-down
```

This stops local processes and containers but retains the PostgreSQL volume. To delete all disposable PostgreSQL data:

```bash
mise exec -- just dev-reset
```

`dev-reset` permanently removes only the named PostgreSQL volume owned by `infra/local/compose.yaml`.

## Common commands

Run `mise exec -- just --list` for the complete command surface. Common checks are:

```bash
mise exec -- just verify
mise exec -- just test-unit
mise exec -- just test-integration
mise exec -- just test-e2e
mise exec -- just codegen-check
mise exec -- just helm-check
mise exec -- just infra-validate
```

`just codegen-check` generates in an isolated temporary tree and never changes tracked files. Infrastructure planning is explicit, for example `mise exec -- just infra-plan development`; it requires the appropriate reviewed credentials and backend access.

Create a feature branch only from a clean worktree:

```bash
mise exec -- just feature my-change
```

The recipe fetches and fast-forwards `development` before creating `feature/my-change`.
