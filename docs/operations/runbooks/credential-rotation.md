# Runbook: credential rotation

## Purpose and scope

Rotate application, database, tunnel, GitHub App, Argo, or provider credentials without exposing values or breaking the supported API/worker/web runtime.

## Safety warnings

- Use the approved secret broker and short-lived operator role; never commit or print secret values.
- Confirm the replacement is valid before revoking the old credential.
- Do not patch live Secret objects as the normal path; reconcile through OpenTofu/External Secrets/GitOps.

## Prerequisites and authorization

Incident/change record, owner approval, secret inventory, target environment, broker access, rollout window, and a tested revocation plan.

## Triggers and symptoms

Scheduled expiry, suspected exposure, provider rejection, failed readiness, tunnel failure, or application connection errors.

## Ordered steps

1. Record the credential class, environment, owner, expiry, and current consumers without copying values.
2. Generate a replacement in the owning provider and store it through the approved broker.
3. Update the External Secret/OpenTofu or protected GitOps input and run the relevant policy/plan checks.
4. Roll the affected API/worker/web or tunnel workload through Argo; observe readiness and durable job convergence.
5. Verify health, database connectivity, provider calls, tunnel access, and telemetry.
6. Revoke the old credential after the grace window and record provider confirmation.

## Verification

Verify `/api/health/live`, `/api/health/ready`, `/api/health/dependencies`, workload readiness, queue age/claims, synthetic access, and absence of secret values in logs/artifacts.

## Rollback or safe stop

If replacement validation fails, stop revocation, restore the prior broker reference through the approved path, and keep the incident/change open until health and audit evidence are complete.

## Escalation

Escalate provider failures to the platform/security owner, suspected exposure to security, database issues to the data owner, and production impact to the incident commander.

## Evidence and audit capture

Retain credential class, provider request IDs, approvals, broker/External Secret revision, rollout/Argo revision, health results, revocation time, and incident timeline; never retain the value.
