# Releases, promotions, and rollback

## Release contract

CI builds the API, worker, and web images plus the Helm chart once from tested development source. Subjects are signed, scanned, attested, digest-pinned, and copied without rebuild through staging and production. Release locks contain the exact tree, migration version, chart, and three application image subjects.

## Promotion

Promotion is a protected GitOps PR. Development may auto-merge after policy; staging requires one approval; production requires two approvals and its protected workflow environment. The promotion workflow verifies the earlier environment’s deployed lock, signatures/referrers/attestations, vulnerability results, and source tree before opening the PR. See [environment promotion](runbooks/promotion.md).

## Verification

After merge, Argo must run the PreSync migration Job, update the API/worker/web workloads, and report healthy/synced. Verify `/api/health/live`, `/api/health/ready`, `/api/health/dependencies`, user-facing synthetic checks, queue convergence, and graph metric freshness.

## Rollback

Rollback is a protected GitOps PR selecting a compatible prior lock. The workflow refuses a migration-version mismatch. If the migration has changed, stop and use a forward fix or isolated restore decision; do not patch live images or run an ad-hoc downgrade. See [rollback](runbooks/rollback.md) and [migration failure](runbooks/migration-failure.md).

## Evidence

Retain source tree, manifest/lock, signatures, approvals, Argo operation, migration result, pod image IDs, health/synthetic results, and any rollback record.
