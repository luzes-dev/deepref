# Runbook: platform bootstrap

## Purpose and scope

Create or adopt encrypted OpenTofu state backends for `development`, `staging`, `production`, and `global`; apply the three isolated AWS environment roots; apply the global Cloudflare/GitHub/Argo root; and establish the protected GitOps handoff. This is a privileged, one-time or recovery procedure, not routine deployment.

## Safety warnings

- Source files are deployable definitions, not proof that any resource exists. The current roots do not yet wire backup, observability, budgets/SNS, admin-runner, or Pod Identity modules; do not claim those controls after applying only the current roots.
- Never use the wrong account or a non-default OpenTofu workspace. Account preconditions are a last defense, not authorization.
- Bootstrap begins with sensitive local state. Use an encrypted, access-controlled ephemeral runner and migrate state immediately.
- Do not commit/copy into evidence populated tfvars, backend configuration, state, saved plans, OAuth/App keys, provider tokens, tunnel tokens, or generated credentials.
- Do not create a public EKS endpoint, public load balancer, bypass DNS name, or permissive Cloudflare policy to simplify bootstrap.
- The protected `gitops` branch must be written through the approved deployment App process. Do not improvise a human direct-push path.

## Prerequisites and authorization

- Approved platform bootstrap change with platform, security, and data owners; production/global steps require the protected environment approvals.
- Three existing AWS accounts, selected global state-anchor account, verified `sa-east-1` quotas, approved SSO administrator profiles, and exact account IDs.
- External populated files for every `infra/bootstrap/<root>/terraform.tfvars.example`, `backend.hcl.example`, and `infra/environments/<root>/terraform.tfvars.example`.
- Cloudflare account/zone/domain/team inputs, GitHub OAuth inputs, deployment App inputs, GitHub organization/team, operations email/budgets/admin CIDRs, and private cluster network path.
- Confirmed design for Argo repository credentials and one-shot tunnel-token delivery.
- Repository checks green. The protected orphan branch and SNS confirmations are allowed to remain pending only if explicitly recorded as blockers before application exposure.

## Triggers and symptoms

- Initial platform creation.
- Re-establishing a missing/corrupt state backend from an approved recovery source.
- Re-running a partially completed bootstrap after a verified safe stop.

## Ordered steps

1. Record the immutable source and validate source contracts:

   ```bash
   git rev-parse HEAD
   git status --short
   mise exec -- just infra-validate
   mise exec -- just helm-check
   bash scripts/check-docs.sh
   ```

2. For each root, select the approved SSO profile and prove identity. Never loop applies across accounts:

   ```bash
   export ROOT=development
   export AWS_REGION=sa-east-1
   export AWS_PROFILE=REPLACE_WITH_DEVELOPMENT_ADMIN_SSO_PROFILE
   export EXPECTED_ACCOUNT_ID=REPLACE_WITH_12_DIGIT_ACCOUNT_ID
   export BOOTSTRAP_TFVARS=/secure/path/development-bootstrap.tfvars
   export BOOTSTRAP_BACKEND=/secure/path/development-bootstrap-backend.hcl

   aws sso login --profile "$AWS_PROFILE"
   test "$(aws sts get-caller-identity --query Account --output text)" = "$EXPECTED_ACCOUNT_ID"
   test "$(tofu -chdir="infra/bootstrap/${ROOT}" workspace show 2>/dev/null || printf default)" = default
   ```

3. On the protected ephemeral runner, initialize local bootstrap state, review a saved plan locally, and apply that exact plan:

   ```bash
   export BOOTSTRAP_PLAN="/tmp/deepref-${ROOT}-bootstrap.tfplan"
   tofu -chdir="infra/bootstrap/${ROOT}" init
   tofu -chdir="infra/bootstrap/${ROOT}" plan \
     -input=false -var-file="$BOOTSTRAP_TFVARS" -out="$BOOTSTRAP_PLAN"
   tofu -chdir="infra/bootstrap/${ROOT}" show -no-color "$BOOTSTRAP_PLAN"
   tofu -chdir="infra/bootstrap/${ROOT}" apply -input=false "$BOOTSTRAP_PLAN"
   tofu -chdir="infra/bootstrap/${ROOT}" output
   ```

4. In the ephemeral checkout, copy `backend.tf.remote.example` to an uncommitted `backend.tf`, then migrate state using the external backend configuration exactly as each bootstrap README states:

   ```bash
   cp "infra/bootstrap/${ROOT}/backend.tf.remote.example" "infra/bootstrap/${ROOT}/backend.tf"
   tofu -chdir="infra/bootstrap/${ROOT}" init \
     -migrate-state -backend-config="$BOOTSTRAP_BACKEND"
   ```

   Verify the remote state object and `.tflock` operation with approved read access. Securely dispose of local state/backup/plan files according to the runner procedure; do not upload them as evidence.

5. Repeat steps 2–4 separately for staging, production, and the chosen global state account. Stop between accounts for peer verification.

6. Populate protected workflow variables/secrets and repository environments from the approved inventory. Confirm the deployment App installation and reviewer team. Run a credential-free speculative plan first:

   ```bash
   gh workflow run "Infrastructure Plan" --ref development -f environment=development
   gh run list --workflow "Infrastructure Plan" --limit 10
   ```

7. Apply per-environment roots in order using full tested source SHAs and protected workflow gates:

   ```bash
   gh workflow run "Infrastructure Apply" --ref development \
     -f environment=development -f source_ref=REPLACE_WITH_FULL_TESTED_SHA -f confirmation=APPLY
   ```

   Use the corresponding trusted source branch/approved SHA for staging and production. After each environment, require workflow success and an empty follow-up plan before proceeding.

8. From the selected global state account and private-cluster-capable runner, dispatch `environment=global` against an approved `main` SHA. This creates Cloudflare/GitHub policy and initial Argo installations but not child workloads or secret values.

9. Through the approved broker, deliver Argo read credentials and Cloudflare tunnel tokens directly to the pre-created containers. Confirm intended SNS recipients have accepted subscriptions after the budgets/SNS module is wired and applied.

10. Establish the protected orphan GitOps tree with exactly the paths enforced by `scripts/ci/validate-gitops-tree.sh`. If the App-only initialization mechanism and branch protection are not yet operational, stop; do not have a human bypass them. Let the development release workflow open the first lock PR.

## Verification

```bash
aws sts get-caller-identity
tofu -chdir="infra/environments/${ROOT}" plan -input=false -lock-timeout=5m -detailed-exitcode
tofu -chdir=infra/environments/global output
kubectl get namespace argocd
argocd app get deepref-root --refresh
```

Exit code `0` is required for an empty OpenTofu plan; `2` means drift/change remains. Verify the three expected Cloudflare hostnames, nonmember denial, member admission, no public AWS origin, Argo healthy/synced, and confirmed alert delivery only after their resources are deployed.

## Rollback or safe stop

Before an apply, close/cancel the change. After a partial apply, do not destroy shared state, KMS, EKS, RDS, Cloudflare, or GitHub policy speculatively. Preserve state locks/evidence, diagnose the exact root, and produce a reviewed corrective plan. Never use `tofu destroy` as bootstrap rollback. If state migration is uncertain, stop all writers and escalate before any `-migrate-state` retry.

## Escalation

Escalate account/state/KMS issues to platform and security owners; RDS or data concerns to the data owner; GitHub/Cloudflare policy problems to security/release owners; any production exposure or lost state to the incident commander.

## Evidence and audit capture

Retain approvals, source SHA/tree, caller ARNs/account IDs, redacted plan summaries, apply run URLs, backend object/version and lock proof, empty plans, Argo output, Access allow/deny tests, no-origin proof, SNS confirmations, and an explicit list of modules/acceptance drills still pending. Never retain state, plan files, or credentials.
