# Pre-production database baseline reset

The application has not entered production. Its unshipped migration history was
consolidated into [one fresh baseline](../crates/app/migrations/0001_baseline.sql).
This deliberately drops upgrade compatibility with databases using the former
149-file chain. The old chain is retained in Git at
`8d1ab733c6c1c11fa545bd82dd8a39b0eee18083`.

## Create the new database

1. Stop the API and workers that use the development database being replaced.
2. Decide whether its data is needed and take a backup if it is. Create a new,
   empty PostgreSQL database or a separately named local database volume. Keep
   the old database until its owner explicitly approves removal.
3. Set `DATABASE_URL` to the new database and apply the baseline:

   ```bash
   cargo sqlx migrate run --source crates/app/migrations --no-dotenv
   cargo sqlx migrate info --source crates/app/migrations --no-dotenv
   ```

   The migration registry should contain successful version `1`. Repeating the
   run is a no-op. Never erase migration registry rows to make an old database
   appear empty.

4. Point the API and workers at the new database. Use the existing bootstrap and
   [database-role provisioning procedure](database-roles-runbook.md). Named
   LOGIN identities and credentials are deployment configuration, not baked into
   the baseline.

No reset command in the repository drops an existing development database or
its volume. Disposable test databases are recreated by their owned test runners;
the template fingerprint changes with the baseline automatically.

## What remains enforced

The schema includes the existing business constraints, tenant references,
scientific history protections, job capability fences, object lifecycle and
privacy proof rules. Rollout activation still requires its actual operational
prerequisites. This reset does not activate commercial metering or establish
production, backup, TLS, cluster or deployment acceptance.

The baseline is generated from a clean database with the former chain applied,
before named roles or operational rollouts are activated. Verification compares
the resulting schema, function bodies/configuration, triggers, constraints,
privileges and required seed rows against that original database. Current-state
tests remain; tests whose sole purpose was upgrading retired database versions
are removed.

The migration-policy report correctly identifies the old files as deleted in
this one reset change. Its append-only rule remains unchanged and applies to
the adopted baseline and subsequent migrations. There is no permanent policy
exception or checksum bypass.
