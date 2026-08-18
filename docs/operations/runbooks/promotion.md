# Runbook: environment promotion

## Purpose and scope

Promote one immutable, signed release from deployed development to staging or from successfully deployed staging to production. The workflow copies exact OCI manifests and referrers, generates an environment release lock, and opens a protected GitOps PR. It never rebuilds.

## Safety warnings

- Source-branch promotion is not deployment proof. Require the earlier environment’s deployed lock and, for production, its successful GitHub deployment record.
- Never hand-edit a release lock, retag an image, substitute a chart version for its digest, or copy only image manifests without signatures/referrers/attestations.
- Do not bypass the GitOps PR, approval count, protected environment, migration hook, or Argo ownership with `kubectl set image` or direct sync changes.
- Stop if another promotion, rollback, migration, restore, or maintenance event is active.

## Prerequisites and authorization

- Release/CI run succeeded for the tested `development` source.
- Digest-pinned release manifest OCI reference and 40-character Git tree are recorded.
- Earlier environment is Argo healthy/synced on that tree; production additionally has the numeric successful staging deployment ID.
- Staging: one authorized GitOps reviewer. Production: two authorized reviewers plus `production-promotion` approval; requester/self-review rules apply.
- Current incident/error-budget/maintenance policy permits promotion.

## Triggers and symptoms

- Approved scheduled promotion of a tested release.
- A validated forward fix that follows the same build-once chain.
- Never use this runbook merely because an environment differs; unexplained drift requires investigation first.

## Ordered steps

1. Inspect the candidate and earlier environment:

   ```bash
   gh run list --workflow Release --limit 10
   argocd app get deepref-root --refresh
   argocd app wait deepref-root --sync --health --timeout 900
   curl --fail --silent --show-error "https://REPLACE_WITH_EARLIER_ACCESS_HOST/api/health/dependencies"
   ```

2. Record `MANIFEST_REFERENCE` and `RELEASE_TREE`; verify their immutable form:

   ```bash
   export MANIFEST_REFERENCE='REPLACE_WITH_REFERENCE@sha256:REPLACE_WITH_64_HEX'
   export RELEASE_TREE='REPLACE_WITH_40_HEX_GIT_TREE'
   [[ "$MANIFEST_REFERENCE" =~ @sha256:[0-9a-f]{64}$ ]]
   [[ "$RELEASE_TREE" =~ ^[0-9a-f]{40}$ ]]
   ```

3. For staging, dispatch:

   ```bash
   gh workflow run "Promote Staging" --ref development \
     -f manifest_reference="$MANIFEST_REFERENCE" \
     -f release_tree="$RELEASE_TREE"
   ```

   For production, first verify the successful deployment ID, then dispatch:

   ```bash
   export STAGING_DEPLOYMENT_ID=REPLACE_WITH_NUMERIC_ID
   gh api "repos/{owner}/{repo}/deployments/${STAGING_DEPLOYMENT_ID}/statuses" --jq '.[0]'
   gh workflow run "Promote Production" --ref development \
     -f manifest_reference="$MANIFEST_REFERENCE" \
     -f release_tree="$RELEASE_TREE" \
     -f staging_deployment_id="$STAGING_DEPLOYMENT_ID"
   ```

4. Follow the workflow. It must verify the signed manifest, earlier lock, copied subjects, signatures, and vulnerability attestations before creating the PR:

   ```bash
   gh run list --workflow "Promote Staging" --limit 5
   gh run list --workflow "Promote Production" --limit 5
   gh run view REPLACE_WITH_RUN_ID --log-failed
   ```

5. Review the App-authored GitOps PR:

   ```bash
   gh pr view REPLACE_WITH_PR --json author,baseRefName,headRefName,files,reviews,statusCheckRollup
   gh pr diff REPLACE_WITH_PR
   ```

   It must target `gitops`, change exactly `environments/<target>/release-lock.yaml`, pass policy, and contain the expected tree/digests/migration version. Obtain required approvals; do not self-approve where prohibited.

6. After merge, observe Argo and the PreSync migration hook from the approved private path:

   ```bash
   argocd app get deepref-root --refresh
   kubectl get jobs --namespace "$NAMESPACE" -l app.kubernetes.io/component=migration
   argocd app wait deepref-root --sync --health --timeout 900
   kubectl get pods --namespace "$NAMESPACE" -o custom-columns='NAME:.metadata.name,IMAGE:.spec.containers[*].image,READY:.status.containerStatuses[*].ready'
   ```

## Verification

Verify `/health/live`, `/health/ready`, `/health/dependencies`, core UI/synthetic, expected release digests, NATS consumer progress, projection state, no new critical alerts, and Access behavior. Production acceptance requires retained evidence, not an interactive observation only.

## Rollback or safe stop

Before merge, close the PR or cancel the workflow. After merge, if migration has not changed and the prior lock is compatible, use [rollback](rollback.md). If the migration hook fails, use [migration failure](migration-failure.md). Do not patch live workloads or repeatedly force Argo sync.

## Escalation

Escalate signature/copy/App/policy failures to release and security owners; migration/schema issues to the data owner; Argo/EKS issues to platform; production core degradation to the incident commander.

## Evidence and audit capture

Retain release/manifest identity, earlier environment proof, dispatch/run, App PR and approvals, policy results, merge/GitOps commit, Argo operation, migration result, pod image IDs, endpoint/synthetic results, alerts, and any safe-stop or rollback.
