# Releases, promotions, and rollback

## Release identity

A merge to `development` that passes `CI` triggers `.github/workflows/release.yml`. It builds the API, worker, projector, web image, and Helm chart once; scans, signs, and attests immutable OCI subjects; publishes a digest-pinned release manifest; then uses the deployment GitHub App to open a development GitOps PR.

Every release lock records the source commit, Git tree, chart repository/version/digest, all four image repositories/digests, migration version, creation time, and recorded OCI referrers. Promotion copies these subjects and referrers; it never rebuilds them.

The source branch ladder (`development -> staging -> main`) and deployed release locks are separate controls. A source merge alone does not deploy, and a GitOps lock must identify a source tree that completed the required earlier environment.

## Normal flow

| Target      | Source evidence                                                             | Workflow                                                | Approval                                       |
| ----------- | --------------------------------------------------------------------------- | ------------------------------------------------------- | ---------------------------------------------- |
| Development | Successful push CI on `development`                                         | Automatic `Release` workflow and `deploy/dev/<tree>` PR | Policy checks; may auto-merge                  |
| Staging     | Digest-pinned development manifest and deployed development tree            | `Promote Staging`                                       | One GitOps approval                            |
| Production  | Same release successfully deployed in staging plus GitHub deployment record | `Promote Production`                                    | Two GitOps approvals and protected environment |

Dispatch examples:

```bash
gh workflow run "Promote Staging" \
  --ref development \
  -f manifest_reference='REPLACE_WITH_DIGEST_PINNED_OCI_REFERENCE' \
  -f release_tree='REPLACE_WITH_40_CHARACTER_GIT_TREE'

gh workflow run "Promote Production" \
  --ref development \
  -f manifest_reference='REPLACE_WITH_DIGEST_PINNED_OCI_REFERENCE' \
  -f release_tree='REPLACE_WITH_DEPLOYED_STAGING_TREE' \
  -f staging_deployment_id='REPLACE_WITH_SUCCESSFUL_DEPLOYMENT_ID'
```

The workflow validates input shape, signed subjects, vulnerability attestations, exact source lock, and prior-environment evidence before opening a PR. Never create or hand-edit a normal release lock outside the workflow.

## Pre-approval review

Review the release manifest and GitOps PR without exposing credentials:

```bash
gh run list --workflow Release --limit 10
gh pr view REPLACE_WITH_GITOPS_PR --json author,baseRefName,headRefName,files,reviews,statusCheckRollup
gh pr diff REPLACE_WITH_GITOPS_PR
scripts/ci/validate-gitops-tree.sh tests/fixtures/gitops-tree.txt
scripts/ci/validate-release-lock.sh --environment staging REPLACE_WITH_RELEASE_LOCK
scripts/ci/verify-release-digests.sh REPLACE_WITH_RELEASE_LOCK
```

For a real candidate, validate a securely obtained lock rather than the fixture. Confirm the PR changes exactly one environment lock, is authored by the configured App bot, and contains no values, Argo definitions, or secret material.

## Deployment verification

After the approved lock merges, Argo is the actuator:

```bash
argocd app get deepref-root --refresh
argocd app wait deepref-root --sync --health --timeout 900
kubectl get jobs,pods,deployments,statefulsets --namespace "$NAMESPACE" -o wide
kubectl get events --namespace "$NAMESPACE" --sort-by=.lastTimestamp
curl --fail --silent --show-error "https://REPLACE_WITH_ACCESS_HOST/api/health/live"
curl --fail --silent --show-error "https://REPLACE_WITH_ACCESS_HOST/api/health/ready"
curl --fail --silent --show-error "https://REPLACE_WITH_ACCESS_HOST/api/health/dependencies"
```

Authenticated HTTP checks require an approved user or synthetic token; never place the token in shell history or evidence. The migration PreSync Job must succeed before new Deployments change. Capture the merged lock, Argo revision, image IDs from pods, health output, and alerts.

## Rollback policy

Rollback is a protected GitOps PR, not `kubectl rollout undo`. The automated workflow only accepts a prior lock with the same migration version as the current release:

```bash
gh workflow run Rollback \
  --ref development \
  -f environment=staging \
  -f target_gitops_commit='REPLACE_WITH_PRIOR_GITOPS_COMMIT' \
  -f expected_current_tree='REPLACE_WITH_CURRENT_40_CHARACTER_TREE' \
  -f confirmation=ROLLBACK
```

Production uses `environment=production` and its protected gate. If migration versions differ, stop: this is a forward-fix or data-recovery decision, not an automatic application rollback. Follow [database migrations](database-migrations.md) and the [migration-failure runbook](runbooks/migration-failure.md).

## Safe stop

Do not merge or continue promotion when any of these is true:

- the source tree differs from the earlier environment;
- a subject is tag-only, unsigned, missing required attestations, or resolves to another digest;
- the migration version is unexpected or compatibility is unreviewed;
- Argo is already degraded or another sync/maintenance operation is active;
- protected approvals, App identity, required checks, SNS/incident coverage, or evidence storage are unavailable;
- production staging-deployment evidence is absent or unsuccessful.

Closing the deployment PR is the normal pre-merge safe stop. After merge, use the rollback workflow only when compatibility permits. Never race Argo with manual image changes.

## Evidence

Retain source CI and release run URLs, manifest OCI reference and signature verification, release-lock before/after, GitOps PR/approvals/merge commit, Argo sync operation, migration Job result, pod image IDs, endpoint/synthetic results, and any rollback record.
