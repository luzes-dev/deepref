# Runbook: migration failure

## Purpose and scope

Respond to a failed PostgreSQL migration PreSync hook while preserving the previously deployed application and database authority. This covers diagnosis and forward repair; it does not authorize schema downgrade or migration-history edits.

## Safety warnings

- Do not repeatedly recreate the Job, run migrations from multiple shells, edit `_sqlx_migrations`, or apply untracked DDL.
- Preserve logs and database metadata before repair.
- Do not roll back across a migration-version boundary or restore over the source database.

## Prerequisites and authorization

Active incident/change record; data owner, service owner, and platform operator; private cluster/database diagnostics; current and target locks; and last-known-good health evidence.

## Triggers and symptoms

Argo is blocked at the migration PreSync Job, the Job fails or times out, readiness reports an incompatible schema, or PostgreSQL reports lock/resource/statement errors.

## Ordered steps

1. Stop promotion and capture Argo/workload state; verify the prior API/worker/web remains available.
2. Capture the migration Job manifest, redacted logs, events, and RDS health without printing secrets.
3. Inspect `_sqlx_migrations`, active locks, and the exact migration source from an approved diagnostic client.
4. Determine whether SQLx rolled back, committed fully, or requires a reviewed additive repair.
5. Fix the source or blocker, test against a fresh database and compatibility fixture, build a new release, and promote normally.
6. Let Argo run one corrected PreSync attempt and observe the rollout.

## Verification

Verify migration metadata, `/api/health/ready`, `/api/health/dependencies`, API/worker/web readiness, queued job convergence, graph metric freshness, RDS locks/latency, and no new critical alert.

## Rollback or safe stop

Leave the failed sync blocked while the previous application serves. Close an unmerged PR or pause releases. Automatic rollback is allowed only when the workflow confirms the same migration version; otherwise use a forward fix or isolated restore decision.

## Escalation

Escalate unknown partial state or data risk to the data owner and incident commander; RDS issues to platform/AWS support; application defects to service maintainers; and exposure to security.

## Evidence and audit capture

Retain source/lock identity, Job status/logs/events, RDS metadata, migration table output, blocker analysis, approvals, corrected release, health/data-invariant results, and incident timeline.
