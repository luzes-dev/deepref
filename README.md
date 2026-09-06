# DeepRef

DeepRef maps article citation networks from seed DOIs. It ships a SvelteKit web
app, a Rust Axum API, a Rust worker, and PostgreSQL state plus graph storage.

## Layout

```text
apps/web        SvelteKit frontend
crates/*        Rust library crates, including the HTTP API and PostgreSQL adapter
services/worker Rust ingestion worker
docs            Architecture, API, and local development
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

See [Local development](docs/local-development.md) for ports, seed data, reset behavior, and the full command surface. See the [documentation index](docs/README.md) for all references.

## Checks

```bash
mise exec -- just verify
mise exec -- just test-unit
mise exec -- just test-integration
mise exec -- just test-e2e
```

Run the TypeScript quality audit directly with:

```bash
mise exec -- pnpm run quality:ts
```

## Development Workflow

Development follows a simple single-branch workflow targeting `main`. Create feature branches and submit pull requests into `main`.

## Containers

```bash
mise exec -- just docker-build
```
