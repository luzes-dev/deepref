# ADR 0001: One server binary with explicit runtime roles

## Status

Accepted.

## Decision

The `deepref-server` binary keeps four runtime roles: `serve`, `worker`, `all`,
and `migrate`. Explicit maintenance commands such as workspace backfill,
legacy-actor decisions, and AI usage reconciliation operate through the same
binary and delegate to the owning library. `serve` starts the API only and
never runs migrations; migrations are an explicit operation.

`all` starts the API and worker together and gives both entrypoints the same
shutdown notification. A signal, or an early exit from either role, asks the
other role to drain and stop as well.

## Consequences

Deployments can use one image while retaining explicit API-only and worker-only
roles. Database migration remains independently auditable and cannot be
triggered accidentally by starting the server.
