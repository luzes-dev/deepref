# ADR 0001: One server binary with explicit runtime roles

## Status

Accepted.

## Decision

The `deepref-server` binary exposes four typed commands: `serve`, `worker`,
`all`, and `migrate`. The commands delegate to the extracted `deepref-api` and
`deepref-worker` entrypoints. `serve` starts the API only and never runs
migrations; migrations are an explicit operation.

`all` starts the API and worker together and gives both entrypoints the same
shutdown notification. A signal, or an early exit from either role, asks the
other role to drain and stop as well.

## Consequences

Deployments can use one image while retaining explicit API-only and worker-only
roles. Database migration remains independently auditable and cannot be
triggered accidentally by starting the server.
