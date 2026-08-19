# DeepRef operations

## Readiness statement

The repository contains application durability work, a production Helm chart, isolated OpenTofu roots, protected release workflows, and observability definitions. That does not prove a platform is deployed. No AWS, Cloudflare, GitHub policy, Argo, or RDS apply/drill evidence is committed in this tree, and the local clone has no `gitops` branch. Production remains blocked until apply-time prerequisites and the [acceptance register](../acceptance/production-platform.md) are signed off.

Use these status terms consistently: **Implemented** means source/configuration exists; **Verified locally** means a local check passed; **Pending apply-time** means deployed infrastructure or an operational drill is required; **Accepted** means reviewed evidence exists.

## Non-negotiable operating model

- PostgreSQL is authoritative for application state, jobs, graph facts, and metric freshness.
- Argo CD owns hosted namespaces, workloads, External Secrets, policies, collectors, and `cloudflared`. Normal operations change those resources through an approved `gitops` PR, not direct workload mutation.
- OpenTofu owns AWS infrastructure, global Cloudflare/GitHub policy, secret containers, Pod Identity associations when wired, and initial Argo installation. It does not own Argo child workloads or secret values.
- `deepref-server serve` is the HTTP role; `deepref-server worker` is the durable job role. A job lease is recoverable after expiry and terminal failures remain persisted for inspection.
- Compose is disposable local PostgreSQL tooling. There is no Compose-based deployment path.
- All users admitted by Cloudflare Access have equal application privileges, including settings and destructive actions. There is no application user, tenant, or ownership model.

## Environments

| Scope | Purpose | State and access | Change path |
| --- | --- | --- | --- |
| `local` | Developer workstation | Disposable loopback PostgreSQL; no cloud identity | `mise exec -- just ...` |
| `development` | First hosted release | Dedicated AWS account, private EKS, Single-AZ RDS; organization-member Access | Build once from tested `development`; App-created GitOps PR may auto-merge |
| `staging` | Production-like verification | Dedicated AWS account, private EKS, Single-AZ RDS | Exact development artifacts; one GitOps approval |
| `production` | User-serving environment | Dedicated AWS account, private EKS, Multi-AZ RDS with deletion protection and 35-day PITR contract | Exact staged artifacts; two GitOps approvals and protected environment |
| `global` | Shared control plane | Selected state-anchor AWS account plus Cloudflare/GitHub/Argo bootstrap providers; not an application runtime | Protected `main` source and `infra-global-apply` |

## Apply-time prerequisites

1. Three AWS accounts, the chosen global state anchor, encrypted backends, KMS/state role ARNs, and cross-account EKS bootstrap roles.
2. A private network path from the global apply runner to every EKS endpoint, plus AWS CLI support for `eks get-token`.
3. Exact EKS cluster names, a reviewed Argo CD chart version, and a protected orphan `gitops` branch.
4. Cloudflare account/zone, base domain, Access policy inputs, tunnel token delivery, and a least-privilege provider token.
5. Deployment GitHub App, reviewer team, protected environments, and exact required checks.
6. Operations escalation recipients, budgets, alert thresholds, Grafana access, and confirmed encrypted notifications.
7. Database, tunnel, synthetic service-token, Argo repository, and application credentials delivered through the approved broker. Do not place them in tfvars, state output, logs, workflow artifacts, or Git.

EKS, RDS, Argo, Cloudflare Access, admission, and recovery drills require deployed infrastructure. Local rendering cannot substitute for those results.

## Guides

- [Backup, restore, and disaster recovery](backup-restore-disaster-recovery.md)
- [Capacity, cost, and maintenance](capacity-cost-maintenance.md)
- [Database migrations](database-migrations.md)
- [Observability, SLOs, and incidents](observability-slos-incidents.md)
- [Ownership, environments, and access](ownership-environments-access.md)
- [Releases, promotions, and rollback](releases-promotions-rollback.md)
- [Security, credentials, and break-glass](security-credentials-break-glass.md)
- [Runbook index](runbooks/bootstrap.md)

## Verification commands

```bash
bash scripts/check-docs.sh
bash scripts/check-api-codegen.sh
bash scripts/helm-check.sh
bash infra/tests/static-contracts.sh
```

Use the [acceptance register](../acceptance/production-platform.md) to record command output, reviewers, and apply-time evidence.
