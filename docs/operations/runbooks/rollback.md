# Runbook: release rollback

## Purpose and scope

Restore staging or production to a previously reviewed release lock through the protected `Rollback` workflow. This runbook covers application/chart rollback only when the target and current release have the same migration version.

## Safety warnings

- Rollback does not downgrade PostgreSQL schema. The workflow intentionally rejects a different migration version.
- Never use `kubectl rollout undo`, direct Argo parameter changes, manual release-lock edits, mutable tags, or deletion of migration history.
- Confirm the target GitOps commit is an ancestor containing an accepted lock for the same environment.
- A rollback can reintroduce a fixed vulnerability or application defect; review the exact prior manifest.

## Prerequisites and authorization

- Active incident/change record, release operator, service owner, and environment approvals. Production uses `rollback-production`.
- Exact target environment (`staging` or `production`), current deployed 40-character source tree, and full prior GitOps commit SHA.
- Target release lock cryptographically valid, known healthy, same migration version, and still available in ECR.
- No concurrent restore, migration repair, promotion, or infrastructure apply.

## Triggers and symptoms

- New release causes application regression while database compatibility remains unchanged.
- Argo reports unhealthy workloads caused by the new artifact/configuration.
- Core or graph behavior regresses and a prior compatible lock is the safest mitigation.

## Ordered steps

1. Open an incident/change and stop further promotions. Capture current state:

   ```bash
   argocd app get deepref-root --refresh
   kubectl get pods,jobs --namespace "$NAMESPACE" -o wide
   curl --silent --show-error "https://REPLACE_WITH_ACCESS_HOST/api/health/dependencies"
   ```

2. Identify the current tree and candidate prior GitOps commit through reviewed GitHub history. Do not copy a local, unreviewed lock:

   ```bash
   export ENVIRONMENT=staging
   export TARGET_GITOPS_COMMIT=REPLACE_WITH_FULL_PRIOR_GITOPS_SHA
   export EXPECTED_CURRENT_TREE=REPLACE_WITH_CURRENT_40_HEX_TREE
   [[ "$TARGET_GITOPS_COMMIT" =~ ^[0-9a-f]{40}$ ]]
   [[ "$EXPECTED_CURRENT_TREE" =~ ^[0-9a-f]{40}$ ]]
   ```

3. Review the prior commit/lock and its original approvals/evidence. Confirm it does not contain the incident cause and has the same migration version.

4. Dispatch the protected workflow:

   ```bash
   gh workflow run Rollback --ref development \
     -f environment="$ENVIRONMENT" \
     -f target_gitops_commit="$TARGET_GITOPS_COMMIT" \
     -f expected_current_tree="$EXPECTED_CURRENT_TREE" \
     -f confirmation=ROLLBACK
   gh run list --workflow Rollback --limit 5
   ```

5. Review the App-authored PR. It must change only the selected lock and pass exact-tree/diff/release validation:

   ```bash
   gh pr view REPLACE_WITH_ROLLBACK_PR --json author,baseRefName,headRefName,files,reviews,statusCheckRollup
   gh pr diff REPLACE_WITH_ROLLBACK_PR
   ```

6. Obtain required approvals and merge. Observe Argo; do not force-sync around failed hooks:

   ```bash
   argocd app get deepref-root --refresh
   argocd app wait deepref-root --sync --health --timeout 900
   kubectl get pods --namespace "$NAMESPACE" -o custom-columns='NAME:.metadata.name,IMAGE:.spec.containers[*].image,READY:.status.containerStatuses[*].ready'
   ```

## Verification

Confirm pod digests equal the target lock, `/health/live` and `/health/ready` succeed, `/health/dependencies` is understood, the triggering symptom is gone, queues/projection converge, synthetics pass, and no new security or data-integrity issue appears.

## Rollback or safe stop

Before PR merge, close it. If the workflow rejects migration compatibility, stop and use a forward fix or [migration failure](migration-failure.md); do not bypass validation. If the rollback worsens impact, preserve both lock identities and prepare a new protected rollback/forward release rather than manually oscillating workloads.

## Escalation

Escalate migration mismatch/data concerns to the data owner; missing/invalid OCI subjects to release/security; Argo/EKS reconciliation issues to platform; sustained production impact to the incident commander.

## Evidence and audit capture

Retain incident reason, current/target trees and migration versions, prior evidence review, workflow/PR/approvals, GitOps commit, Argo sync, target pod digests, health/synthetic/alert results, and decision timeline.
