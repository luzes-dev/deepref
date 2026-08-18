# Runbook: break-glass access

## Purpose and scope

Grant and use temporary elevated operational access during an active incident when normal protected GitOps/OpenTofu paths cannot meet the immediate safety need. Break-glass is a bounded exception, never a parallel administration model.

## Safety warnings

- Break-glass does not authorize destructive database operations, security-control bypass, public exposure, secret disclosure, or unreviewed production changes.
- Do not create static AWS keys, share sessions/kubeconfigs, disable Cloudflare Access, Argo self-heal, signature admission, audit logs, encryption, RDS deletion protection, or backups.
- Use read-only actions first. Direct workload mutation is a last resort; Argo may revert it.
- Never fight Argo reconciliation. If a direct action cannot remain safe under the owner, pause and use an emergency App-authored GitOps PR.
- Every session must expire and be reviewed; convenience is not a trigger.

## Prerequisites and authorization

- Active incident ID, named incident commander, exact environment/resource/action/reason, and defined session expiry.
- Security owner and platform/data/service owner appropriate to the action. Production should have two-person authorization unless responder availability makes that impossible; record the exception.
- Approved short-lived SSO break-glass role pre-created with CloudTrail/EKS/Kubernetes audit coverage and emergency contact path.
- Evidence recorder who will redact secrets and a rollback/reconciliation owner.

## Triggers and symptoms

- Normal deployment/IaC path unavailable or too slow for an imminent data/security/availability hazard.
- Locked-out private EKS or provider control plane where a pre-approved emergency role is required for diagnosis/containment.
- Suspected compromise requiring immediate credential/session containment.
- Not valid: routine deployment, skipped approval, operator convenience, or bypassing a failed policy check.

## Ordered steps

1. Open/update the incident. Record authorization, exact proposed action, expected effect, safe stop, expiry, and why normal ownership cannot meet the need.

2. Assume the approved short-lived SSO role. Never export static credentials:

   ```bash
   export AWS_PROFILE=REPLACE_WITH_BREAK_GLASS_SSO_PROFILE
   export AWS_REGION=sa-east-1
   aws sso login --profile "$AWS_PROFILE"
   aws sts get-caller-identity
   ```

   Compare the account ID and role ARN to the incident authorization. Stop on mismatch.

3. Obtain cluster context only if in scope:

   ```bash
   export CLUSTER_NAME="$(tofu -chdir="infra/environments/${ENVIRONMENT}" output -raw eks_cluster_name)"
   aws eks update-kubeconfig --region "$AWS_REGION" --name "$CLUSTER_NAME" \
     --alias "break-glass-${ENVIRONMENT}"
   kubectl config current-context
   kubectl auth can-i --list --namespace "$NAMESPACE"
   ```

4. Capture read-only baseline before mutation:

   ```bash
   argocd app get deepref-root --refresh
   kubectl get pods,deployments,statefulsets,jobs --namespace "$NAMESPACE" -o wide
   kubectl get events --namespace "$NAMESPACE" --sort-by=.lastTimestamp
   curl --silent --show-error "https://REPLACE_WITH_ACCESS_HOST/api/health/dependencies"
   ```

5. Reconfirm the one authorized action with the incident commander. Execute only that action, against explicit identifiers, with a peer observing. Do not use broad globs, loops across environments, `tofu destroy`, stream/PVC deletion, or database restore/delete commands unless a separate runbook and data authorization explicitly require them.

6. Verify the expected safety effect and unintended impact immediately. If ineffective, stop; do not expand scope without new authorization.

7. Restore the normal owner as soon as safe:

   - propose/merge an emergency deployment-App GitOps PR for any workload state that must persist;
   - propose/apply a reviewed OpenTofu correction for cloud resources;
   - revert temporary direct state that should not persist;
   - let Argo reach synced/healthy without manual drift.

8. End the SSO session, revoke temporary grants/tokens, remove any temporary EKS access entry through its owner, and confirm expiry/audit events.

## Verification

Verify the incident hazard is contained, authoritative data is understood, Argo/OpenTofu ownership is reconciled, application/dependency health is acceptable, no public bypass/control disablement remains, temporary access is revoked, audit logs captured, and follow-up source changes are assigned.

## Rollback or safe stop

Stop before mutation if identity/scope/peer/evidence is missing. If the action is ineffective or harmful, execute only its pre-approved inverse, then reassess. Do not improvise a larger destructive action. Preserve data/queue/volume state and escalate.

## Escalation

Escalate scope expansion to the incident commander and appropriate owner; suspected compromise to security; data-risk actions to the data owner; provider lockout to AWS/Cloudflare/GitHub support; inability to revoke access to security leadership.

## Evidence and audit capture

Retain incident/authorizers/exception reason, caller/session/account/context, exact redacted commands and identifiers, before/after state, UTC timeline, normal-path failure, reconciliation PR/plan, revocation proof, provider audit event IDs, deviations, and post-incident actions. Never retain session tokens, kubeconfigs, Secrets, or credentials.
