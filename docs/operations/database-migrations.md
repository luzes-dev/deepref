# Database migrations

## Contract

PostgreSQL is authoritative. Migrations under `services/api/migrations` are append-only. `deepref-api serve` never migrates; `deepref-api migrate` is the only application migration command. In hosted environments the Helm chart runs it as an Argo `PreSync` Job with `backoffLimit: 0` and a bounded active deadline, so failure stops the new sync before application Deployments change.

Local commands are:

```bash
mise exec -- just migrate
mise exec -- just test-integration
```

The current required schema version is defined by code and exposed at `GET /health/ready`. Never infer compatibility only from a migration filename.

## Migration design policy

1. Use expand/contract changes. Land additive schema and dual-compatible code before removing an old shape.
2. Do not rewrite already-shipped migration files. Add a new ordered migration.
3. Avoid long blocking DDL. State a lock-timeout, statement-timeout, expected table size, index strategy, and abort signal in the review.
4. Make application behavior backward-compatible for the complete promotion/rollback window.
5. Treat destructive data change as a separately authorized data operation with a tested restore path.
6. Keep Neo4j constraints and indexes in `crates/graph/migrations`; the projector applies them idempotently. Neo4j data remains rebuildable from PostgreSQL.
7. A rollback may select only a release with the same migration version through the current workflow. Schema downgrade is not automated.

The accountable data owner must define the compatibility window, migration timeout, acceptable lock budget, and cleanup date for each nontrivial migration. These are change-specific decisions, not repository-wide defaults.

## Review checklist

- Migration is additive or has a documented multi-release contract.
- Queries used by the old and new application versions both work after migration.
- Default/backfill behavior is bounded and observable.
- Index creation and constraints are safe for expected production volume.
- Backup/PITR health was checked and a restore drill is current.
- `services/api/tests/migration.rs`, affected worker/projector tests, and Helm migration tests pass.
- Release lock migration version and rollback compatibility are understood.
- Production approval identifies a data owner and safe-stop authority.

## Hosted observation

Do not create the Job manually. Merge the approved release lock and observe the Argo-owned hook:

```bash
argocd app get deepref-root --refresh
kubectl get jobs --namespace "$NAMESPACE" -l app.kubernetes.io/component=migration
kubectl logs --namespace "$NAMESPACE" \
  -l app.kubernetes.io/component=migration \
  --all-containers --tail=200
kubectl get events --namespace "$NAMESPACE" --sort-by=.lastTimestamp
```

Logs may contain database error context; redact before retention. Never print `DATABASE_URL_FILE`, mount contents, or Secret values.

## Verification

After a successful hook:

```bash
curl --fail --silent --show-error "https://REPLACE_WITH_ACCESS_HOST/api/health/ready"
kubectl get deployments --namespace "$NAMESPACE" -o wide
argocd app wait deepref-root --sync --health --timeout 900
```

If authorized database access is required, connect through the approved private runner and query only metadata needed for verification. Do not use the master credential for routine application validation.

## Failure and rollback boundaries

On PreSync failure, do not delete/retry the Job repeatedly or bypass the hook. The prior application should remain serving; verify it, capture failure evidence, and follow [migration failure](runbooks/migration-failure.md). Fix the source and create a new release, or perform an explicitly authorized forward data repair.

Never:

- downgrade schema ad hoc;
- restore production over the existing instance;
- mark a failed SQLx migration successful manually;
- patch the Deployment to skip readiness/schema checks;
- run `deepref-api migrate` from multiple shells “until it works.”

## Evidence

Retain the migration review, source/release/GitOps identities, pre-change recovery-point health, hook start/end and status, redacted logs, readiness schema response, lock/latency observations, and any forward-fix or safe-stop decision.
