# Security

## Report a vulnerability

Report suspected vulnerabilities privately. Use the repository Security tab's private vulnerability reporting flow when it is enabled. If it is unavailable, contact the maintainers through an already-established private channel and ask for a secure reporting path before sending exploit details. Do not open a public issue, discussion, or pull request containing a vulnerability or credential.

Include the affected source commit or immutable artifact digest, impact, reproduction conditions, and a private contact method. Do not access data that is not yours, disrupt a hosted environment, or publish secrets as proof. Maintainers must configure and publish a durable private security contact before production launch.

The supported production version is the latest release deployed from `main`. Security fixes follow the repository branch ladder (`development -> staging -> main`) or the documented `hotfix/* -> main` path with back-merges. Development and staging branches are testing inputs, not separately supported production versions.

## Application access model

Cloudflare Access is the application perimeter for development, staging, and production. Admission is based on membership in the configured GitHub organization. There is no application authentication, user, organization, tenant, ownership, or role model.

Every identity admitted by Cloudflare has equal application privileges, including settings and destructive actions. A Cloudflare application session does not grant AWS, GitHub, Kubernetes, Argo, database, or observability access. Those systems require separate least-privilege SSO roles and approvals.

The perimeter must remain fail closed: no public AWS application load balancer, origin IP, alternate hostname, disabled origin JWT validation, permissive temporary Access policy, or shared-password bypass is supported. Use the [Cloudflare or IdP outage runbook](docs/operations/runbooks/cloudflare-or-idp-outage.md).

## Credentials and secrets

- Use AWS IAM Identity Center/SSO and GitHub OIDC for short-lived cloud access. Do not create or distribute static AWS access keys.
- Keep provider tokens, OAuth secrets, GitHub App private keys, tunnel tokens, Argo repository credentials, database credentials, synthetic tokens, and kubeconfigs out of Git, tfvars, state outputs, logs, chat, and workflow artifacts.
- OpenTofu creates secret containers, not secret values. Runtime values are delivered through the approved broker/Secrets Manager and External Secrets. Do not patch Kubernetes Secrets as the normal path.
- Rotate credentials through the [credential rotation runbook](docs/operations/runbooks/credential-rotation.md). The security owner must approve routine intervals, emergency revocation, and time-bounded exceptions before production.

## Supply chain and vulnerabilities

Application and chart releases are digest-pinned, scanned, signed, and attested, then promoted without rebuild. Do not retag, substitute, or bypass signature/admission checks. A vulnerability exception requires security-owner approval, an expiry, compensating controls, and a tracked remediation.

Never include real credentials or sensitive production data in a reproduction. If a secret may have been exposed, rotate it immediately and treat the event as an incident even if the underlying defect is not yet confirmed.

## Break-glass boundaries

Break-glass access is limited to an active incident when normal protected GitOps/OpenTofu paths cannot meet an immediate safety need. It requires a named incident commander, explicit scope/environment/action/expiry, appropriate security/platform/data authorization, short-lived SSO access, audit capture, and immediate revocation/reconciliation.

Break-glass does not authorize disabling Cloudflare Access, signature admission, Argo ownership, audit logging, encryption, RDS deletion protection, backups, or data-integrity controls. Direct workload mutation is never the normal operational path. Follow the [break-glass runbook](docs/operations/runbooks/break-glass-access.md).

See [Security, credentials, and break-glass](docs/operations/security-credentials-break-glass.md) for the full operating contract.
