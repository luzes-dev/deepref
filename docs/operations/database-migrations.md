# Database migrations

## Authority and compatibility

SQLx migrations under `crates/postgres/migrations` are the only schema authority. Migration 0008 completes the infrastructure collapse: durable jobs, UUID graph reads, metric recomputation, and PostgreSQL graph freshness are supported from the same database. Migration 0009 adds generic acquisition provenance and source-record metadata. Migrations 0001 through 0009 must apply in order and remain idempotent at the SQL operation level.

`deepref-server migrate` is the only supported migration command in local, CI, and hosted workflows. The Helm migration Job runs as an Argo PreSync hook before application Deployments. Normal `serve` and `worker` roles never migrate.

## Release compatibility

- Additive schema changes must support the previous deployed application until rollout completes.
- A migration that changes data shape requires importer/worker/API compatibility review and a fixture proving deterministic UUID graph semantics.
- Never edit `_sqlx_migrations`, mark a failed migration successful, or apply untracked production DDL.
- Rollback across a migration boundary is not automatic; use a forward fix or isolated restore decision.

## Local and CI checks

```bash
cargo fmt --all --check
cargo test --workspace --locked
DATABASE_URL=postgres://postgres:deepref@127.0.0.1:55432/deepref_pr2_review \
  cargo test -p deepref-postgres --test graph --locked
bash scripts/check-api-codegen.sh
bash scripts/helm-check.sh
```

The graph fixture must include reports without DOI, internal and outbound UUID edges, deterministic bounds/order, legacy rank parity, fresh-import metrics, and later recomputation freshness. Apply migrations to a fresh disposable database for acceptance evidence.

## Failure handling

Stop the release when the PreSync Job fails. Preserve job logs, migration metadata, database blockers, and the prior workload state; then follow [migration failure](runbooks/migration-failure.md).
