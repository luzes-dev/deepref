# Ownership, environments, and access

## Ownership boundaries

| Owner | Responsibilities |
| --- | --- |
| Application/data | Rust API, worker, PostgreSQL schema, jobs, graph facts, metrics, migrations, and API contract |
| Platform | EKS, RDS, networking, KMS, backups, telemetry, capacity, and incident response |
| Release/security | CI, signatures, vulnerability policy, release locks, GitHub App, and protected environments |
| Perimeter | Cloudflare Access, Tunnel, DNS, and synthetic access checks |
| GitOps | Hosted namespace workloads, External Secrets, policies, collectors, values, and immutable release locks |

No resource may have two owners. OpenTofu installs Argo but does not manage Argo child workloads; it creates secret containers but never secret values.

## Environment access

Use AWS IAM Identity Center/SSO and GitHub OIDC for short-lived access. Production database and cluster access is private, audited, time-bounded, and approved by the incident/change owner. Cloudflare Access admits organization members to the application; application identities do not grant infrastructure access.

## Change paths

- Infrastructure: reviewed OpenTofu plan/apply from the protected environment.
- Hosted application: App-authored GitOps PR with release lock, required checks, and environment-specific approvals.
- Local: mise/Just/Compose/Process Compose on a disposable workstation.
- Emergency: named break-glass incident with scope, expiry, audit capture, and reconciliation.

## Evidence

Retain access review, caller/account, approvals, GitOps/App identity, plan/lock, Argo revision, and post-change health evidence. Never commit credentials, state, kubeconfigs, or populated variable files.
