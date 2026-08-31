# Runbook: environment promotion

## Purpose and scope

Promote one immutable, signed API/worker/web/chart release from development to staging or successfully deployed staging to production. The workflow copies exact OCI subjects and opens a protected GitOps PR; it never rebuilds.

## Safety warnings

- Never hand-edit a release lock, retag an image, substitute a chart digest, or bypass the GitOps PR/approval/migration hook.
- Stop if another promotion, rollback, migration, restore, or maintenance event is active.

## Prerequisites and authorization

Signed digest-pinned manifest, 40-character source tree, earlier environment health/deployed lock, required reviewer(s), protected environment, and current incident/error-budget approval.

## Triggers and symptoms

Approved scheduled promotion or validated forward fix from the tested development tree.

## Ordered steps

1. Verify the candidate manifest, source tree, earlier Argo revision, and `/api/health/dependencies`.
2. Dispatch the environment promotion workflow with immutable inputs.
3. Verify signatures, attestations, copied subjects, vulnerability results, and App-authored GitOps PR.
4. Review the exact environment lock and approvals; merge only after required checks pass.
5. Observe Argo’s migration PreSync Job and API/worker/web rollout.

## Verification

Verify `/api/health/live`, `/api/health/ready`, `/api/health/dependencies`, expected digests, API/worker/web readiness, queued job convergence, graph metric freshness, Access behavior, and no new critical alert.

## Rollback or safe stop

Before merge, close the PR or cancel the workflow. After merge, use [rollback](rollback.md) only for a compatible migration version; use [migration failure](migration-failure.md) for a failed hook. Never patch live workloads.

## Escalation

Escalate signature/copy/App/policy failures to release/security, migration issues to the data owner, Argo/EKS issues to platform, and production degradation to the incident commander.

## Evidence and audit capture

Retain manifest/tree, workflow/run, App PR/approvals, policy results, merge/GitOps revision, Argo operation, migration result, pod image IDs, endpoint results, alerts, and safe-stop/rollback record.
