# DeepRef

DeepRef is an open-source workspace for systematic reviews built to use AI without giving up traceability.

AI works from the evidence in your review, with the documents, citations, protocol, and review state kept alongside every task. Its output is treated as a proposal, not as scientific truth. You can inspect the evidence behind it, review the result, and decide what becomes part of the review.

Around that model, DeepRef covers the rest of the workflow: importing and deduplicating literature, title and abstract screening, full-text review, study grouping, appraisal, extraction, PRISMA, citation-graph exploration, recommendations, and repeatable automations.

The goal is not to automate researchers out of the process. It is to make more of the review computable while keeping the reasoning and evidence inspectable.

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
