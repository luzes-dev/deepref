# Security, credentials, and break-glass

## Trust model

Cloudflare Access is the only application perimeter. The global OpenTofu source creates a GitHub identity provider, organization-membership policy, one Access application/tunnel/DNS record per environment, origin-side Access JWT validation, and a fail-closed tunnel catch-all. There is no public AWS load balancer or intended bypass hostname.

The application deliberately has no login, users, organizations, tenants, ownership, or privilege tiers. Every identity admitted by Cloudflare receives equal application privileges, including settings and destructive actions. Access membership and GitHub organization governance are therefore security-critical.

## Credential classes

| Credential                           | Storage/delivery rule                                                                  | Rotation owner               |
| ------------------------------------ | -------------------------------------------------------------------------------------- | ---------------------------- |
| AWS operator access                  | IAM Identity Center/SSO and short-lived role session                                   | Platform/security owner      |
| GitHub deployment App key            | Protected GitHub secret; App token minted per workflow                                 | Security/release owner       |
| Cloudflare provider/OAuth secret     | Approved broker; sensitive provider input may enter encrypted global state             | Security owner               |
| Tunnel token                         | Retrieved out-of-band and written directly to pre-created environment secret container | Platform/security owner      |
| Database master/runtime credentials  | AWS-managed/Secrets Manager; runtime consumed through External Secrets                 | Data/security owner          |
| Synthetic Access token               | Protected synthetic store, bounded scope, monitored and rotated                        | Observability/security owner |
| Argo repository credential           | Read-only, delivered out-of-band when private                                          | Platform/security owner      |

Never commit credentials, real tfvars/backend files, state, saved plans, kubeconfigs, `.env`, tokens, private keys, database URLs, or generated Kubernetes Secrets. Do not pass secrets on command lines when an environment/file/broker path is available; shell history and process listings are evidence surfaces.

## Rotation policy

The organization must approve rotation intervals and exception handling before production. Rotate immediately on suspected disclosure, personnel/ownership change, provider warning, or scope change. Routine rotation follows [credential rotation](runbooks/credential-rotation.md) and must overlap old/new credentials when the provider supports it.

Rotation order is producer/provider -> secret broker/Secrets Manager -> External Secrets convergence -> dependent health -> old credential revocation. Do not restart all dependencies simultaneously, and do not patch Kubernetes Secrets directly because the controller will reconcile them.

## Vulnerability response

Report vulnerabilities through the private channel in the root `SECURITY.md`; never require a public proof of concept. Triage must record affected artifact digests/environments, exploitability, data exposure, containment, remediation release, promotion path, and disclosure decision.

Release workflows scan HIGH/CRITICAL findings and produce vulnerability attestations. An exception requires security-owner approval, expiry, compensating controls, and a tracked remediation. Never retag or rebuild only one environment to bypass a finding; fix source and promote one new signed release.

## Break-glass boundary

Break-glass is for an active incident when protected GitOps/OpenTofu paths cannot meet the safety need. It does not bypass data ownership, security reporting, production evidence, or peer authorization.

Required controls:

- active incident/ticket and named incident commander;
- explicit target environment/resource/action and expiry;
- two-person production authorization when two approvers are reachable;
- short-lived SSO role/session, no static key creation;
- smallest reversible action; read-only first;
- command/audit capture with secret redaction;
- immediate revocation and reconciliation into the owning source after stabilization.

Direct workload mutation is never the normal path. Do not disable Argo self-heal, signature policy, Access, audit logging, encryption, deletion protection, or backup retention merely to make an incident easier. Follow [break-glass access](runbooks/break-glass-access.md).

## Cloudflare or IdP failure

Fail closed. Do not add a public origin, temporary DNS bypass, permissive Access policy, or shared application password. Validate Cloudflare, GitHub IdP, tunnel replicas, and origin health separately. Core availability behind a denied perimeter is still an incident; it is not authorization to expose the origin. Use the [Cloudflare or IdP outage runbook](runbooks/cloudflare-or-idp-outage.md).

## Audit and review

At an agreed cadence review Cloudflare policy/IdP, GitHub organization membership, App installation/permissions, protected environments, AWS SSO/IAM and EKS entries, Grafana roles, secret age, certificate expiry, synthetic-token scope, break-glass sessions, and vulnerability exceptions.

Retain provider audit identifiers and redacted metadata. Never retain secret material as “proof.”
