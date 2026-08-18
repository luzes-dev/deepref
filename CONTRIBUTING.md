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
mise trust
mise install
mise exec -- just bootstrap
```

## Checks

```bash
mise exec -- just verify
mise exec -- just test-unit
mise exec -- just docs-check
```

Run Fallow before larger TypeScript changes:

```bash
mise exec -- pnpm run quality:ts
```

## CI/CD

GitHub Actions runs Rust, web, E2E, TypeScript quality, Docker build, and security checks on pull requests to `development`, `staging`, and `main`.

Merges to `development` build signed immutable images and charts once in ECR. Protected promotion workflows copy the exact tested digests to staging and production and update reviewed GitOps release locks; promotion never rebuilds an artifact.

Operational changes must preserve the ownership boundaries in [Production operations](docs/operations/README.md). Hosted workload changes are proposed through GitOps automation; do not document or normalize direct `kubectl` mutation. Documentation that changes platform behavior or acceptance evidence must update [AC-01 through AC-16](docs/acceptance/production-platform.md) without marking apply-time work complete from local checks.

## Repository Settings

Set the GitHub default branch to `development`.

Protect `development` with required pull requests, one approval, required CI checks, stale approval dismissal, and no force pushes.

Protect `staging` with required pull requests, one approval, required CI and security checks, stale approval dismissal, and no force pushes.

Protect `main` with required pull requests, two approvals, required CI and security checks, conversation resolution, stale approval dismissal, and no force pushes.
