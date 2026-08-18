# Runbook: Cloudflare or identity-provider outage

## Purpose and scope

Diagnose and respond when Cloudflare Access, GitHub identity, DNS, Tunnel, or origin-side Access validation prevents legitimate users from reaching an otherwise healthy environment. Preserve the fail-closed, tunnel-only perimeter.

## Safety warnings

- Never create a public AWS load balancer, alternate/bypass hostname, direct origin DNS record, permissive Access rule, shared password, or application auth shortcut.
- Do not disable origin-side Access JWT validation or change the fail-closed tunnel catch-all.
- Equal application privileges mean broadening Access immediately broadens settings/destructive access.
- Provider status and user reports do not prove origin health; test layers separately.

## Prerequisites and authorization

- Incident record, incident commander, security owner, platform operator, and communications owner.
- Read access to Cloudflare/GitHub status, global OpenTofu outputs, DNS, Access/Tunnel audit, Argo/Kubernetes, and approved synthetic credentials.
- Any policy/DNS/tunnel change requires security approval and protected global OpenTofu/GitOps workflow; production follows its approval gates.

## Triggers and symptoms

- Organization members receive Access denial/login loop; nonmembers unexpectedly pass.
- GitHub OAuth/organization membership checks fail.
- DNS/TLS/tunnel errors, `DeepRefTunnelReplicaShortfall`, certificate alert, or synthetic outage.
- Origin is healthy via cluster-local checks but public Access path fails.

## Ordered steps

1. Classify affected environments/users and verify public behavior without leaking a token:

   ```bash
   export ACCESS_HOST=REPLACE_WITH_ENVIRONMENT_HOSTNAME
   dig +short "$ACCESS_HOST"
   curl --head --silent --show-error "https://${ACCESS_HOST}/"
   tofu -chdir=infra/environments/global output cloudflare_hostnames
   tofu -chdir=infra/environments/global output cloudflare_tunnel_ids
   ```

2. Check Cloudflare and GitHub provider status through approved channels. Record incident identifiers; do not wait on providers before checking the origin.

3. From the private cluster path, inspect `cloudflared`, web, and API without modifying them:

   ```bash
   argocd app get deepref-root --refresh
   kubectl get pods,deployments --namespace "$NAMESPACE" -o wide
   kubectl logs --namespace "$NAMESPACE" \
     -l app.kubernetes.io/component=cloudflared --tail=500
   kubectl get endpoints --namespace "$NAMESPACE"
   ```

4. Distinguish the layer:

   - DNS/TLS: verify the only record points to `<tunnel-id>.cfargotunnel.com` and remains proxied.
   - Tunnel: verify expected replicas (one development, two staging/production), token secret synchronization, outbound connectivity, and tunnel audit.
   - Access/GitHub IdP: verify the configured GitHub IdP, organization-membership policy, application audience, OAuth callback/secret age, and membership using provider audit—not screenshots alone.
   - Origin: verify web/API service/endpoints and cluster-local health from an approved diagnostic mechanism; do not expose it publicly.

5. If source drift or credential expiry is confirmed, rotate credentials through [credential rotation](credential-rotation.md) or apply a reviewed global OpenTofu/GitOps correction. Do not patch Cloudflare resources manually and leave IaC drift.

6. If Cloudflare/GitHub has a provider outage, remain fail closed, communicate impact, monitor origin health, and wait/retest. Business pressure does not authorize a bypass.

7. Run both authorized-member success and nonmember denial synthetics after recovery. A member success alone is insufficient.

## Verification

Verify DNS is tunnel-only, TLS valid, expected tunnel replicas connected, Argo healthy/synced, authorized member succeeds, nonmember is denied, origin JWT validation remains on, application health succeeds through Access, and there is no public AWS load balancer/IP/bypass record.

## Rollback or safe stop

Close/revert unmerged IaC/GitOps changes when diagnosis changes. If a correction worsens access, restore the prior reviewed configuration through its owner. Remain unavailable rather than fail open. Never keep a temporary Access exception after the incident; if an exception was explicitly authorized, expire and audit it immediately.

## Escalation

Escalate provider outages to Cloudflare/GitHub support and communications; policy/OAuth/token issues to security; tunnel/EKS networking to platform; unexpected nonmember access as a security incident; extended production access loss to leadership/incident commander.

## Evidence and audit capture

Retain affected scope/timeline, provider incident IDs, DNS/TLS/tunnel metadata, Access audit events, membership/denial test IDs, redacted logs, source/plan/PR/apply identities, no-origin proof, communications, and exception revocation. Never capture OAuth secrets, tokens, cookies, or JWTs.
