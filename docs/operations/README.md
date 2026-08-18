# Production operations

This suite is the operating contract for DeepRef. It describes the repository as implemented and separates source-level readiness from evidence that can exist only after privileged infrastructure deployment.

## Readiness statement

The repository contains application durability work, a production Helm chart, isolated OpenTofu roots, protected release workflows, and observability definitions. That does **not** prove a platform is deployed. No AWS, Cloudflare, GitHub policy, Argo, RDS, NATS, or Neo4j apply/drill evidence is committed in this tree, and the local clone has no `gitops` branch. Production remains blocked until the apply-time prerequisites and [acceptance register](../acceptance/production-platform.md) are signed off.

Use these status terms consistently:

- **Source implemented**: the repository contains an inspectable contract or automated test.
- **Locally verified**: a command ran against source or disposable local dependencies in the current change.
- **Apply-time pending**: credentials, deployed infrastructure, protected settings, or a real drill are required.
- **Accepted**: retained evidence has been reviewed by the accountable human owner. Documentation alone never produces this status.

## Non-negotiable operating model

- PostgreSQL is authoritative. Neo4j Community is a single-node, asynchronous read model that may be cleared and rebuilt.
- Argo CD owns hosted namespaces, workloads, NATS, Neo4j, External Secrets, policies, collectors, and `cloudflared`. Normal operations change those resources through an approved `gitops` PR, not direct workload mutation.
- OpenTofu owns AWS infrastructure, global Cloudflare/GitHub policy, secret containers, Pod Identity associations when wired, and initial Argo installation. It does not own Argo child workloads or secret values.
- The deployment GitHub App is the only normal writer to the protected orphan `gitops` branch. Development may auto-merge after policy; staging requires one approval; production requires two approvals plus the protected environment gate.
- Hosted services consume pre-provisioned NATS streams and durable consumers. Only the Helm bootstrap Job administers them. `scripts/bootstrap-local-nats.sh` is local-only.
- Compose is disposable local dependency tooling. There is no Compose-based deployment path.
- All users admitted by Cloudflare Access have equal application privileges, including settings and destructive actions. There is no application user, tenant, or ownership model.

## Environments

| Scope         | Purpose                      | State and access                                                                                              | Change path                                                                |
| ------------- | ---------------------------- | ------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| `local`       | Developer workstation        | Disposable loopback PostgreSQL, one-node NATS, and Neo4j; no cloud identity                                   | `mise exec -- just ...`                                                    |
| `development` | First hosted release         | Dedicated AWS account, private EKS, Single-AZ RDS; organization-member Access                                 | Build once from tested `development`; App-created GitOps PR may auto-merge |
| `staging`     | Production-like verification | Dedicated AWS account, private EKS, Single-AZ RDS, three-replica NATS contract                                | Exact development artifacts; one GitOps approval                           |
| `production`  | User-serving environment     | Dedicated AWS account, private EKS, Multi-AZ RDS with deletion protection and 35-day PITR contract            | Exact staged artifacts; two GitOps approvals and protected environment     |
| `global`      | Shared control plane         | Selected state-anchor AWS account plus Cloudflare/GitHub/Argo bootstrap providers; not an application runtime | Protected `main` source and `infra-global-apply`                           |

Total loss of `sa-east-1` is outside the declared service/AZ recovery target. The platform is greenfield; no production import from an earlier deployment is planned.

## Apply-time prerequisites

Before any bootstrap or hosted drill, an authorized owner must record and approve:

1. Three existing AWS accounts, their 12-digit IDs, `sa-east-1` quotas, and the chosen global state-anchor account.
2. AWS IAM Identity Center/SSO administrator sessions for the one-time backend, KMS, and GitHub OIDC bootstrap in every account.
3. State bucket names, backend keys/configuration, KMS administrators, EKS access roles, VPC CIDRs, admin CIDRs, and the private runner/network path to every EKS API.
4. Cloudflare account and zone IDs, base domain, Zero Trust team slug, GitHub OAuth client ID/secret, provider token, and tunnel-token delivery process.
5. Deployment GitHub App ID, installation, private key, permissions, reviewer team, exact required checks, protected environments, and App-only GitOps rules.
6. Operations email/escalation recipients, monthly budgets, alert thresholds, Identity Center Grafana users, and confirmed encrypted SNS subscriptions. An unconfirmed SNS subscription is not operational.
7. Populated database, NATS, Neo4j, tunnel, synthetic service-token, Argo repository, and application credentials delivered through the approved broker. Do not place them in tfvars, state output, logs, workflow artifacts, or Git.
8. A reviewed, pinned Argo CD chart version and an already-created protected orphan `gitops` branch with the exact allowlisted tree.

EKS, RDS, NATS, Neo4j, Argo, Cloudflare Access, admission, and recovery drills require deployed infrastructure. Local rendering cannot substitute for those results.

## Guides

1. [Ownership, environments, and access](ownership-environments-access.md)
2. [Releases, promotions, and rollback](releases-promotions-rollback.md)
3. [Database migrations](database-migrations.md)
4. [Backup, restore, and disaster recovery](backup-restore-disaster-recovery.md)
5. [Observability, SLOs, and incident response](observability-slos-incidents.md)
6. [Security, credentials, and break-glass](security-credentials-break-glass.md)
7. [Capacity, cost, and maintenance](capacity-cost-maintenance.md)

The `runbooks/` directory contains exactly twelve executable procedures, beginning with [platform bootstrap](runbooks/bootstrap.md). Acceptance evidence is tracked one-for-one in [AC-01 through AC-16](../acceptance/production-platform.md).

## Standard hosted preflight

Run from an approved workstation or the private administration runner. Use an environment-specific AWS SSO profile; never export static AWS keys.

```bash
export ENVIRONMENT=staging
export AWS_REGION=sa-east-1
export AWS_PROFILE=REPLACE_WITH_APPROVED_SSO_PROFILE
export NAMESPACE="deepref-${ENVIRONMENT}"

aws sts get-caller-identity
export CLUSTER_NAME="$(tofu -chdir="infra/environments/${ENVIRONMENT}" output -raw eks_cluster_name)"
aws eks update-kubeconfig --region "$AWS_REGION" --name "$CLUSTER_NAME" --alias "deepref-${ENVIRONMENT}"
kubectl config current-context
kubectl auth can-i get pods --namespace "$NAMESPACE"
kubectl get pods --namespace "$NAMESPACE"
argocd app get deepref-root --refresh
```

Stop if the caller account, cluster, namespace, release lock, or authorization does not match the approved change. Production mutation requires the relevant runbook and approvals; `kubectl edit`, `kubectl set image`, ad-hoc scaling, and manual Job creation are not normal release mechanisms.

## Evidence and audit capture

Every hosted change or drill must retain, outside Git when sensitive:

- ticket/incident and approver identities;
- UTC start/end, environment, caller ARN, source commit, Git tree, GitOps commit, and release-lock digest set;
- reviewed workflow run and PR links;
- redacted command output, Argo health/sync export, Kubernetes events, dashboards, and alert delivery;
- observed RPO/RTO or rebuild duration and parity data when applicable;
- deviations, safe-stop/rollback decisions, and follow-up owners.

Use a restricted evidence store with an agreed retention period. Never capture tokens, credentials, connection strings, OAuth secrets, private keys, session cookies, or unredacted Kubernetes Secrets.
