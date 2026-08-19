# Production infrastructure

This tree implements isolated OpenTofu roots for three AWS accounts plus one global control root. OpenTofu workspaces are forbidden. State is encrypted and versioned in S3, and every remote backend uses native `.tflock` locking with `use_lockfile = true`; there is no DynamoDB lock table.

## Ownership

| Owner | Resources |
| --- | --- |
| Per-environment OpenTofu roots | VPC, private EKS, RDS, ECR, KMS, secret containers, Pod Identity, backups, budgets, observability, and private administration/bootstrap support |
| Global OpenTofu root | Cloudflare GitHub IdP, Access policy/applications, Tunnel/DNS configuration, GitHub rulesets/environments, the `argocd` namespace and pinned Argo CD Helm release, and one root Application per cluster |
| Argo/GitOps | Workload namespaces, application workloads, External Secrets, policies, collectors, `cloudflared` Deployments, ApplicationSets, environment values, and immutable release locks |
| Versioned application migrations | PostgreSQL schema, graph constraints, and graph metrics |
| Out-of-band credential delivery | Cloudflare/GitHub provider tokens, OAuth client secret, deployment App key, Argo read credential, tunnel tokens, and populated secret values |

No resource may have two owners. In particular, OpenTofu installs Argo but does not manage Argo child workloads; it creates Cloudflare tunnels but does not manage `cloudflared` pods; and it creates secret containers but never secret values.

## Bootstrap and apply workflow

1. Decide which of the three existing AWS accounts anchors global state. For development, staging, production, and global, run the corresponding `infra/bootstrap/<root>` once with approved SSO administrator credentials and an external variable file. Verify the exact caller account before plan/apply.
2. Migrate each bootstrap to its encrypted S3 backend using its external partial backend configuration. Confirm remote state and `.tflock` writes, then securely remove local state and backups. Never use a non-default workspace.
3. Apply `infra/environments/development`, then staging, then production from each account's protected runner. Review a no-save plan, apply the reviewed identity, and require a subsequent empty plan.
4. Complete the global prerequisites below. From the selected state account, use the declared cross-account cluster roles and a runner with network access to all private EKS endpoints. Apply `infra/environments/global` only from the protected `infra-global-apply` environment on a tested main commit.
5. Deliver the Argo repository read credential when needed and retrieve each Cloudflare tunnel token directly into its pre-created secret container. Do not expose either value through OpenTofu output, logs, workflow artifacts, or committed files.
6. Bootstrap the protected orphan `gitops` branch through the separate reviewed runbook, then verify Argo is healthy/synced, Access admits an organization member, Access denies a nonmember, DNS has no bypass record, and the AWS accounts have no public application load balancer.

Normal changes follow `feature/* -> development -> staging -> main`. Hotfixes follow `hotfix/* -> main` and are back-merged to staging and development. The deployment GitHub App alone proposes `deploy/* -> gitops` changes. Development locks may auto-merge after policy checks; staging requires one team approval; production requires two plus its protected workflow environment.

## Apply-time prerequisites and decisions

The following values and capabilities must exist before apply; examples contain placeholders only:

- Three distinct AWS account IDs, the chosen global state-anchor account, globally unique state bucket names, KMS/state role ARNs, and cross-account EKS bootstrap roles with matching EKS access entries.
- A private network path from the global apply runner to every EKS endpoint, plus AWS CLI support for `eks get-token`.
- Exact EKS cluster names and an exact reviewed Argo CD chart version. The `gitops` branch and each configured root path must exist before Argo can sync.
- A Cloudflare account/zone, base domain, Zero Trust team-domain slug, and a least-privilege provider token supplied through the provider's standard environment variable.
- A GitHub OAuth App whose callback is the Cloudflare-provided redirect URL. Its client secret necessarily enters encrypted OpenTofu state through the provider and must be injected through the approved secret broker, never committed.
- An installed deployment GitHub App with the required repository permissions, its numeric App ID, an organization reviewer team, exact required check-run names, and a GitHub plan that supports repository rulesets, path-specific required reviewers, and protected environments.
- A reviewed decision for private-repository Argo read authentication and a one-shot tunnel-token delivery mechanism. Neither credential is managed or output here.
- Existing GitHub OIDC/trusted roles and protected workflow environments for plans, applies, release, promotion, and rollback. Provider tokens, App private keys, and OAuth/tunnel secrets must be configured as protected secrets, not tfvars.

Do not commit populated tfvars/backend files, state, lock backups, saved plans, credentials, generated secrets, tunnel tokens, provider lockfiles created incidentally, or orphan-branch contents.

## Verification

Use the pinned toolchain where available:

```sh
tofu fmt -check -recursive infra
tofu -chdir=infra/modules/argo-bootstrap test
tofu -chdir=infra/modules/cloudflare-perimeter test
tofu -chdir=infra/modules/github-repository test
tofu -chdir=infra/bootstrap/global init -backend=false
tofu -chdir=infra/bootstrap/global validate
tofu -chdir=infra/bootstrap/global test
tofu -chdir=infra/environments/global init -backend=false
tofu -chdir=infra/environments/global validate
tofu -chdir=infra/environments/global test
infra/tests/static-contracts.sh
```

Provider-backed validation requires registry access. A speculative plan also requires read-only AWS, Cloudflare, GitHub, and private-cluster connectivity; never save or upload it.
