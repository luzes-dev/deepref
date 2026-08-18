# Runbook: migration failure

## Purpose and scope

Respond to a failed PostgreSQL migration PreSync hook while preserving the previously deployed application and database authority. This runbook covers diagnosis, safe stop, and forward repair; it does not authorize schema downgrade or manual migration-history edits.

## Safety warnings

- Do not repeatedly delete/recreate the Job, run `deepref-api migrate` from multiple shells, or bypass the Argo hook.
- Do not edit `_sqlx_migrations`, mark a failed migration successful, disable readiness, or apply ad-hoc DDL without data-owner review.
- A partially applied nontransactional operation must be proven, not assumed. Preserve logs and database metadata before repair.
- Do not roll back the application across a migration-version boundary or restore over the source RDS instance.

## Prerequisites and authorization

- Active change/incident record; data owner, service owner, and platform operator assigned.
- Read access to Argo/Kubernetes and approved private database diagnostic access. Production repair requires explicit data-owner and protected-environment authorization.
- Current/target release locks, source migration files, pre-change PITR status, and last known-good application evidence.
- No concurrent promotion, rollback, restore, or migration attempt.

## Triggers and symptoms

- Argo sync is blocked at the `migration` PreSync Job.
- Migration Job is `Failed` or exceeds `activeDeadlineSeconds`.
- `/health/ready` reports an older/incompatible schema after a release attempt.
- RDS logs/metrics show lock timeout, statement failure, connection loss, or resource exhaustion during migration.

## Ordered steps

1. Stop promotion and capture Argo/current workload state. Confirm the previous Deployment is still serving:

   ```bash
   argocd app get deepref-root --refresh
   kubectl get jobs,pods,deployments,replicasets --namespace "$NAMESPACE" -o wide
   curl --silent --show-error "https://REPLACE_WITH_ACCESS_HOST/api/health/live"
   curl --silent --show-error "https://REPLACE_WITH_ACCESS_HOST/api/health/ready"
   ```

2. Capture the failed hook and events without printing Secrets:

   ```bash
   kubectl describe job --namespace "$NAMESPACE" \
     -l app.kubernetes.io/component=migration
   kubectl logs --namespace "$NAMESPACE" \
     -l app.kubernetes.io/component=migration \
     --all-containers --tail=500
   kubectl get events --namespace "$NAMESPACE" --sort-by=.lastTimestamp
   ```

3. Inspect RDS health and PITR before any repair:

   ```bash
   aws rds describe-db-instances --region "$AWS_REGION" \
     --db-instance-identifier "ambient-scribes-${ENVIRONMENT}" \
     --query 'DBInstances[0].{Status:DBInstanceStatus,Latest:LatestRestorableTime,Storage:AllocatedStorage,Pending:PendingModifiedValues}'
   ```

4. From the approved private database client using a broker-delivered, non-master diagnostic role, inspect migration metadata and blockers. Configure connection through the approved `PGSERVICEFILE`; do not put a URL/password on the command line:

   ```bash
   psql service=deepref-diagnostic --set ON_ERROR_STOP=1 \
     --command='SELECT version, description, installed_on, success, checksum FROM _sqlx_migrations ORDER BY version;'
   psql service=deepref-diagnostic --set ON_ERROR_STOP=1 \
     --command='SELECT pid, wait_event_type, wait_event, state, left(query,160) FROM pg_stat_activity WHERE datname=current_database();'
   ```

5. Compare the error to the exact new migration under `services/api/migrations`. Determine whether SQLx rolled the migration back, it committed fully, or a reviewed idempotent forward repair is required. Record the conclusion and reviewer.

6. Choose one safe path:

   - **Source defect, no incompatible commit**: fix/add an append-only migration or compatible code, run local/integration/Helm checks, build one new release, and promote normally.
   - **Transient capacity/lock issue**: remove the blocker through its owning process, then create a new GitOps sync/release attempt under data-owner approval. Do not loop retries.
   - **Partial change/data risk**: stop user-changing operations if necessary, take/verify a recovery point, test a forward repair on a restored or staging database, then execute once under a reviewed data procedure.
   - **Potential corruption/loss**: escalate to [RDS failover/PITR](rds-failover-pitr.md); do not alter the source further.

7. After the corrected release/repair is approved, let Argo run the PreSync Job. Observe one attempt and then the normal rollout.

## Verification

```bash
argocd app wait deepref-root --sync --health --timeout 900
curl --fail --silent --show-error "https://REPLACE_WITH_ACCESS_HOST/api/health/ready"
curl --fail --silent --show-error "https://REPLACE_WITH_ACCESS_HOST/api/health/dependencies"
kubectl get jobs,pods,deployments --namespace "$NAMESPACE" -o wide
```

Verify migration metadata, old/new query compatibility, ingestion/outbox processing, projection lag, RDS locks/latency, and no new critical alert.

## Rollback or safe stop

The default safe stop is leaving the failed sync blocked while the previous application serves. Close an unmerged GitOps PR or pause further releases; do not mutate live workload specs. Automatic release rollback is allowed only if the workflow confirms the same migration version. Otherwise use a forward fix or isolated PITR decision.

## Escalation

Escalate suspected data loss/corruption or unknown partial state immediately to the data owner and incident commander. Escalate RDS/resource issues to platform/AWS support; application migration defects to service maintainers; security-sensitive data exposure to the security owner.

## Evidence and audit capture

Retain source/GitOps/release identities, failed Job manifest/status, redacted logs/events, RDS/PITR metadata, `_sqlx_migrations` output, blocker analysis, chosen path/approvers, corrected release/repair, readiness and data-invariant results, and incident timeline.
