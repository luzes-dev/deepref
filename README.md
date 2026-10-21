# DeepRef

DeepRef is an open-source workspace for systematic reviews built to use AI without giving up traceability.

AI works from the evidence in your review, with the documents, citations, protocol, and review state kept alongside every task. Its output is treated as a proposal, not as scientific truth. You can inspect the evidence behind it, review the result, and decide what becomes part of the review.

Around that model, DeepRef covers the rest of the workflow: importing and deduplicating literature, title and abstract screening, full-text review, study grouping, appraisal, extraction, PRISMA, citation-graph exploration, recommendations, and repeatable automations.

The goal is not to automate researchers out of the process. It is to make more of the review computable while keeping the reasoning and evidence inspectable.

## Layout

```text
apps/web        SvelteKit frontend
crates/*        Rust library crates, including the HTTP API and PostgreSQL adapter
services/worker Rust durable-job worker
docs            Architecture, API, and local development
```

## Security model

DeepRef is a single-user application for a trusted machine. The API has no authentication: anyone who can reach it can read, change, and delete every project, run workflows, and spend the configured AI key. Actor ids in requests are self-asserted, so they label work but do not prove who did it.

- The API binds `127.0.0.1:8080` by default and refuses a non-loopback address unless `DEEPREF_ALLOW_NETWORK_BIND=1` is set.
- It rejects requests addressed to an unknown `Host` or port, which blocks DNS rebinding from web pages you visit. Loopback names use the API bind port; `API_CORS_ORIGINS` adds browser/proxy authorities (including their ports). Set it to the site's public origin when using Caddy or a published container port. Health probes must use an allowed Host too.
- HTTP clients act only as `user`; automation and system actors are created by the server itself.
- `APP_ENV` is required and must be `development` or `production`; an unset value stops startup. Developer machines use `APP_ENV=development`, every deployed service uses `APP_ENV=production` with its fail-fast checks.

Never expose the API, or the web image that proxies to it, to a network without an authenticating proxy in front. The API container image sets `DEEPREF_ALLOW_NETWORK_BIND=1` only so that such a proxy can reach it. See [SECURITY.md](SECURITY.md) to report a vulnerability.

## Runtime contracts

Workflows, purpose-built automations, and compiled reviews share `durable_runs`, `durable_steps`, `durable_attempts`, and the fenced `durable_run_step` job. Review manifests retain their scientific identity and history; exhausted claims settle before a new retry generation opens. Worker lookups use live leases, independently of reviewer-facing AI-first visibility.

Slack, arbitrary HTTP, webhook triggers, and inbound e-mail nodes are parked by default (`DEEPREF_WORKFLOW_NICHE_NODES=false`). Existing graphs remain readable. Enabling this explicit flag restores authoring and ingress; a flag-off worker rejects queued niche graphs before their effects. Outgoing e-mail and purpose-built automations remain available.

DOI acquisition uses canonical `acquisition_items`, report memberships, and `report_fetch_state`; migrations 0080–0081 backfill and remove the legacy DOI plane. New acquisition depths are bounded to 0–3. Previously valid operator/project defaults above 3 are retained with historical provenance, not silently clamped; update such defaults before starting a new acquisition without an explicit supported depth.

Full-text citation links resolve through bounded queue and evidence pages. A later-page evidence error shows a retry action rather than partial overlays. Exclusion reasons stay bound to the displayed report, and held decision keys cannot decide unseen reports.

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
