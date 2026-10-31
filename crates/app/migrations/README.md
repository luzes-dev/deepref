# Migrations

`0001_baseline.sql` is the fresh-database baseline for the pre-production
application. It replaces the 149 unshipped migrations present at commit
`8d1ab733c6c1c11fa545bd82dd8a39b0eee18083`.

The baseline contains the final schema, functions, triggers, indexes, constraints,
PUBLIC privilege restrictions and required seed rows. Seed creation timestamps
and the platform usage bucket's UTC month are evaluated when it runs. Operational
rollout controls remain inactive; creating a database does not attest a fleet
drain or activate privacy, object-key or commercial usage features.

This is a reset boundary, not an upgrade migration. A database that applied the
old chain must be replaced by a new empty database. Do not clear its
`_sqlx_migrations` table or run the baseline over its existing objects. Existing
development data must be backed up or explicitly discarded by its owner. See the
[reset procedure](../../../docs/database-baseline-reset.md).

SQLx applies files in version order and stores SHA-384 checksums in
`_sqlx_migrations`. Once this baseline is adopted, applied migration files are
immutable. Add future changes as `0002_description.sql`, `0003_description.sql`,
and so on, with `-- phase: expand` or `-- phase: contract` headers. Contract
migrations must name the release that must be out of service.

The retired SQL remains in Git history. Historical upgrade-only tests were
retired with this reset; current application and database-contract tests remain.
The small resources under `crates/app/schema/role_contracts` and
`crates/review/schema-contracts` are verification/provenance inputs, not another
executable migration chain.
