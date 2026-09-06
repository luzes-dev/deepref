# Contributing

## Branch Workflow

The repository uses a single-branch workflow targeting `main`.

Create feature work from `main`:

```bash
git fetch origin
git checkout main
git pull --ff-only
git checkout -b feature/<short-description>
```

Open pull requests into `main`.

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
```

Run Fallow before larger TypeScript changes:

```bash
mise exec -- pnpm run quality:ts
```

## CI/CD

GitHub Actions runs Rust, web, E2E, TypeScript quality, Docker build, and security checks on pull requests to `main`.
