# Ownership, environments, and access

## Accountability before production

The repository can express technical ownership but cannot assign people. Before the first hosted apply, leadership must name primary and backup humans for these roles and replace the temporary personal fallback in `.github/CODEOWNERS` with organization teams:

| Operating role     | Accountable for                                                 | May authorize                                          |
| ------------------ | --------------------------------------------------------------- | ------------------------------------------------------ |
| Service owner      | API, web, worker, projector behavior and application SLO        | Application release and application incident decisions |
| Platform owner     | AWS, EKS, Helm, Argo, NATS, Neo4j, Cloudflare Tunnel            | Infrastructure plans, maintenance, scaling, drains     |
| Data owner         | PostgreSQL migrations, backup, restore, data correctness        | Migration and restore drills; production data recovery |
| Security owner     | Cloudflare Access, IAM/SSO, credentials, vulnerability response | Credential rotation, access exceptions, break-glass    |
| Incident commander | Cross-service coordination and communications                   | Incident actions within delegated scope                |
| Finance owner      | Budgets, forecasts, cost exceptions                             | Capacity/cost changes above agreed thresholds          |

No single-person availability assumption is acceptable for production. A requester must not be the sole production approver. GitHub protected environments are the authorization record for workflow operations; the incident record is the authorization record for emergency operations.

## Technical ownership boundaries

| Resource                  | Source path                                         | Runtime owner                                  | Prohibited overlap                                                       |
| ------------------------- | --------------------------------------------------- | ---------------------------------------------- | ------------------------------------------------------------------------ |
| Rust services and schema  | `services/**`, `crates/**`                          | Release artifact; schema through migration Job | API `serve` never migrates                                               |
| Hosted workloads          | `charts/deepref/**`, GitOps environment values/lock | Argo CD                                        | OpenTofu and operators do not patch workload specs                       |
| NATS streams/consumers    | `charts/deepref/templates/nats-bootstrap-job.yaml`  | Argo/Helm bootstrap Job                        | API/worker/projector credentials cannot administer JetStream             |
| Neo4j constraints/indexes | `crates/graph/migrations/**`                        | Projector commands                             | No standalone unmanaged constraint file                                  |
| AWS resources             | `infra/environments/**`, `infra/modules/**`         | OpenTofu                                       | Argo does not own AWS resources                                          |
| Argo installation/root    | `infra/modules/argo-bootstrap/**`                   | OpenTofu bootstrap                             | Child Applications remain GitOps-owned                                   |
| Cloudflare/GitHub policy  | `infra/environments/global`, relevant modules       | Global OpenTofu root                           | Workload charts do not create perimeter/policy resources                 |
| Secret values             | Approved broker plus Secrets Manager                | Security/data owners                           | OpenTofu creates containers only; Git and chart values contain no values |

Service accountability must also be explicit in the on-call catalog:

| Service                              | Primary operating role                      | Dependency/escalation boundary                                  |
| ------------------------------------ | ------------------------------------------- | --------------------------------------------------------------- |
| Web gateway and Svelte SPA           | Service owner                               | Cloudflare/Tunnel and EKS issues go to platform/security        |
| Axum API                             | Service owner                               | PostgreSQL/schema to data owner; perimeter to platform/security |
| Ingestion worker                     | Service owner                               | JetStream/platform, provider, and PostgreSQL/data escalation    |
| Graph projector                      | Service owner                               | Neo4j/platform and PostgreSQL/data escalation                   |
| PostgreSQL/RDS                       | Data owner with platform operator           | AWS service/AZ recovery and PITR                                |
| NATS JetStream                       | Platform owner                              | Application poison/retry behavior returns to service owner      |
| Neo4j Community                      | Platform owner with projector service owner | Rebuild/parity requires data-owner approval                     |
| Argo, EKS, Cloudflare, observability | Platform owner                              | Access/credential incidents escalate to security                |

Several reusable modules are present but the per-environment roots currently instantiate only KMS, network, secret containers, ECR, EKS, and RDS. Backup, budgets/SNS, observability, admin-runner, and Pod Identity modules must be wired, reviewed, and applied before their controls can be treated as active.

## Environment isolation

`development`, `staging`, and `production` use separate AWS accounts, OpenTofu roots, state, EKS clusters, RDS instances, ECR repositories, Secrets Manager containers, and Cloudflare hostnames. OpenTofu workspaces are forbidden; every root contains an account guard and requires the default workspace.

`global` is not a fourth runtime. It is a separately locked state root in a deliberately selected account and manages shared Cloudflare/GitHub policy plus initial Argo installations. An operator must assume cross-account cluster roles from a runner that can reach private EKS APIs.

Local state is disposable and loopback-only:

```bash
mise exec -- just dev
mise exec -- just dev-down
mise exec -- just dev-reset
```

`just dev-reset` permanently deletes only the named local dependency volumes. It must never be adapted to a hosted context.

## Access levels

1. **Read-only observer**: dashboards, logs, `kubectl get`, Argo read, OpenTofu state outputs that are not sensitive.
2. **Release operator**: may dispatch promotion/rollback workflows but cannot bypass protected approvals.
3. **Platform operator**: may run reviewed OpenTofu plans/applies and maintenance from protected environments.
4. **Data recovery operator**: time-limited permission for RDS recovery and approved database validation.
5. **Break-glass operator**: temporary, incident-bound elevation under the [break-glass runbook](runbooks/break-glass-access.md).

Cloudflare Access admission is separate from AWS/GitHub operational access. Every admitted application user has equal in-app privileges. Do not infer that an application session authorizes AWS, Kubernetes, Argo, database, or GitHub actions.

## Access request and review

- Require SSO group membership, least privilege, an expiry for temporary roles, and approval by the accountable owner.
- Prefer EKS access entries and IAM Identity Center roles. Do not distribute kubeconfig files or static IAM access keys.
- Keep provider tokens, OAuth secrets, GitHub App private keys, tunnel tokens, database credentials, and NATS credential files in the approved broker.
- Review SSO, GitHub App, protected-environment, Cloudflare, Grafana, and EKS access at an agreed cadence and after personnel changes.
- Export only identifiers for audits. Redact secret ARNs when they reveal sensitive organization structure; never export secret values.

Read-only AWS identity and cluster preflight:

```bash
aws sts get-caller-identity
tofu -chdir="infra/environments/${ENVIRONMENT}" workspace show
tofu -chdir="infra/environments/${ENVIRONMENT}" output -raw eks_cluster_name
kubectl auth can-i --list --namespace "$NAMESPACE"
```

Expected workspace is `default`. Stop on any account or environment mismatch.

## Production access boundaries

- Production source comes from `main`; production artifacts must already be deployed successfully in staging.
- Production promotion requires two GitOps approvals and the `production-promotion` environment. Production rollback uses `rollback-production`.
- Infrastructure apply uses `infra-production-apply` or `infra-global-apply`; the workflow verifies the full source SHA belongs to `main`.
- Normal operators do not directly mutate Deployments, StatefulSets, Jobs, NATS stream definitions, ExternalSecrets, or Argo Applications.
- Emergency direct mutation requires break-glass authorization, explicit incident logging, the smallest reversible change, and immediate reconciliation back into the owning source. If Argo ownership would revert the action, pause and choose an approved GitOps change instead of fighting reconciliation.

## Access evidence

For every quarterly review and elevated session, retain reviewer, subject, role/group, environment, reason, grant/expiry, removed access, and the provider audit-event identifiers. Access review itself does not prove application acceptance or infrastructure health.
