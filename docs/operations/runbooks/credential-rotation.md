# Runbook: credential rotation

## Purpose and scope

Rotate database, NATS, Neo4j, Cloudflare tunnel/OAuth/provider, GitHub App, Argo repository, or synthetic Access credentials while preserving least privilege, External Secrets ownership, and auditability.

## Safety warnings

- Never print, diff, commit, upload, or paste old/new credential values. Secret value is not evidence.
- Do not patch Kubernetes Secrets directly; External Secrets will reconcile them from the approved store.
- Do not revoke the old credential before the new credential is delivered and verified unless active compromise requires immediate containment.
- Rotate one credential class/environment at a time. Simultaneous database, NATS, tunnel, and App rotation destroys diagnostic isolation.
- Do not use long-lived static AWS access keys for rotation.

## Prerequisites and authorization

- Approved rotation/incident record, credential owner and dependent service owner, security approval, and a tested rollback/overlap mechanism.
- Exact environment, secret container/property, consumers, TTL/expiry, and provider audit trail identified.
- Short-lived SSO/provider session and approved secret broker. Production requires protected approval and monitoring coverage.
- Current Argo/External Secrets/dependent service health and no unrelated deployment or maintenance.

## Triggers and symptoms

- Scheduled rotation, upcoming expiry, personnel/scope change, provider mandate, or cryptographic policy change.
- Suspected/confirmed disclosure, unauthorized use, failed authentication, or secret-age alert.
- Certificate/tunnel/synthetic/App credential nearing expiry.

## Ordered steps

1. Inventory metadata only and record dependents. Do not retrieve values for evidence:

   ```bash
   aws sts get-caller-identity
   tofu -chdir="infra/environments/${ENVIRONMENT}" output secret_arns
   kubectl get externalsecrets --namespace "$NAMESPACE"
   kubectl get pods --namespace "$NAMESPACE" -o wide
   argocd app get deepref-root --refresh
   ```

2. Define validation before generating the new credential:

   - database: new least-privilege role/secret connects with TLS and application queries;
   - NATS: subject-scoped publish/consume works and administration is denied;
   - Neo4j: encrypted private connection and expected database/query works;
   - tunnel/Access: expected tunnel replicas plus member allow/nonmember deny;
   - GitHub App: token mint, exact repository permissions, App-authored test PR path;
   - Argo repository: read/sync only;
   - synthetic token: canary access only, bounded expiry/scope.

3. Create the new version in the owning provider using its approved process. For Secrets Manager-backed values, write directly from the broker/session without echoing the payload. Record only provider secret/version IDs and audit event.

4. Wait for the configured External Secrets refresh and observe reconciliation:

   ```bash
   kubectl get externalsecrets --namespace "$NAMESPACE" \
     -o custom-columns='NAME:.metadata.name,READY:.status.conditions[?(@.type=="Ready")].status,REFRESH:.status.refreshTime'
   kubectl get events --namespace "$NAMESPACE" --sort-by=.lastTimestamp
   ```

   If an expedited refresh is necessary, use an approved controller-supported GitOps/source procedure. Do not annotate/patch Secrets ad hoc as the default.

5. Use the owning rollout mechanism only if the consumer cannot reload credentials. Application/chart rollout requires an immutable release/GitOps PR; controller/provider configuration requires OpenTofu/GitOps. Avoid restarting all pods simultaneously.

6. Execute the predefined validation and observe auth failures, dependency health, queue/projection progress, and tunnel/synthetic behavior through at least the agreed overlap window.

7. Revoke/disable the old credential at the provider. For suspected compromise, prioritize containment, then restore service with the new value. Confirm the old credential fails in a safe non-destructive test where supported.

8. Review and remove unintended grants, temporary sessions, old secret versions according to retention policy, and any emergency exception.

## Verification

Verify External Secrets Ready, dependent pods healthy, `/health/dependencies` acceptable, NATS consumers/projection progressing, tunnel/member/nonmember synthetics as relevant, provider audit shows new active/old revoked, and no authentication-error alert. GitHub App rotation must prove protected App identity without merging an unintended deployment.

## Rollback or safe stop

Before revocation, stop and keep the old credential if new validation fails; repair delivery without exposing values. After revocation, re-enable the old version only if provider/security policy permits and security owner authorizes it. Otherwise issue another new credential. Revert configuration through OpenTofu/GitOps; never restore a copied secret from chat/evidence.

## Escalation

Escalate suspected disclosure to security/incident commander; database rotation to data owner; NATS/Neo4j/tunnel to platform; GitHub App/release blockage to release/security; provider failures to vendor support.

## Evidence and audit capture

Retain authorization, credential class/environment, provider/secret/version identifiers, consumer inventory, UTC creation/convergence/revocation times, External Secrets status, validation IDs/results, source/PR/apply identities, old-credential rejection, access review, and exceptions. Never retain credential content.
