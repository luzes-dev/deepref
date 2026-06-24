# Contributing

## Branch Workflow

The repository uses three long-lived branches:

- `development`: default branch and integration branch for daily work.
- `staging`: pre-production branch promoted from `development`.
- `main`: production branch promoted from `staging`.

Create feature work from `development`:

```bash
git fetch origin
git checkout development
git pull --ff-only
git checkout -b feature/<short-description>
```

Open pull requests in this order:

1. `feature/*`, `fix/*`, `bugfix/*`, `chore/*`, `docs/*`, `refactor/*`, or `test/*` into `development`.
2. `development` into `staging`.
3. `staging` into `main`.

Production hotfixes may use `hotfix/*` branches directly into `main`. After a hotfix merges, open follow-up PRs from `main` into `staging` and `development` so the fix is not lost in later promotions.

## Setup

```bash
pnpm install
docker compose -f infra/docker-compose.yml up -d
```

## Checks

```bash
pnpm run ci
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Run Fallow before larger TypeScript changes:

```bash
pnpm run quality:ts
```

## CI/CD

GitHub Actions runs Rust, web, E2E, TypeScript quality, Docker build, and security checks on pull requests to `development`, `staging`, and `main`.

Merges to long-lived branches publish GHCR images:

- `development`: `development` and `development-<short-sha>`
- `staging`: `staging` and `staging-<short-sha>`
- `main`: `production`, `production-<short-sha>`, and `latest`

Version tags matching `v*.*.*` also publish release images and create a GitHub release.

## Repository Settings

Set the GitHub default branch to `development`.

Protect `development` with required pull requests, one approval, required CI checks, stale approval dismissal, and no force pushes.

Protect `staging` with required pull requests, one approval, required CI and security checks, stale approval dismissal, and no force pushes.

Protect `main` with required pull requests, two approvals, required CI and security checks, conversation resolution, stale approval dismissal, and no force pushes.
