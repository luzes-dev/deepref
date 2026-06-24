# DeepRef

DeepRef maps article citation networks from seed DOIs. It ships a SvelteKit web
app, a Rust Axum API, a Rust ingestion worker, PostgreSQL state, NATS JetStream,
and Neo4j graph infrastructure.

## Layout

```text
apps/web        SvelteKit frontend
crates/*        Rust library crates
services/api    Rust HTTP API
services/worker Rust ingestion worker
infra           Local infrastructure
docs            Architecture and API notes
examples        Self-hosting examples
```

## Requirements

- Node 24
- pnpm 11.3.0
- Rust 1.95.0
- Docker

## Setup

```bash
pnpm install
docker compose -f infra/docker-compose.yml up -d
```

## Development

```bash
pnpm run dev:web
cargo run -p deepref-api
cargo run -p deepref-worker
```

The API defaults to `postgres://deepref:deepref@localhost:5432/deepref` and
`nats://localhost:4222`.

## Checks

```bash
pnpm run ci
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Run the TypeScript quality audit directly with:

```bash
pnpm run quality:ts
```

## Development Workflow

Daily work happens through pull requests into `development`. Promote tested work with PRs from `development` to `staging`, then from `staging` to `main`. Emergency `hotfix/*` branches may target `main` directly, followed by back-merge PRs into `staging` and `development`.

## Containers

```bash
pnpm run docker:build
pnpm run compose:config
```

Tagged releases publish:

- `ghcr.io/<owner>/deepref-api`
- `ghcr.io/<owner>/deepref-worker`
- `ghcr.io/<owner>/deepref-web`

Merges to `development`, `staging`, and `main` publish environment-tagged images to the same repositories.
