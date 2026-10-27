# Database roles and audit retention

## Symptom

Production API/worker startup refuses an inactive or privileged database login,
or audit retention reports remaining eligible rows. Development fixtures may
use their isolated migration owner; production startup never accepts that owner
as runtime. Role checks are catalog checks, not an environment attestation.

## Query

Use the corresponding protected database credential and run:

```bash
deepref-server database-roles check runtime
deepref-server database-roles check migration
deepref-server database-roles check maintenance
```

A check verifies activation, capability membership, absence of administrative
attributes/unrelated role memberships/admin options, schema/database creation,
ownership, audit/proof/migration mutation and privileged function access. Every
migration connection sets `ROLE deepref_migration_owner` before DDL. API/worker
production startup checks the runtime contract before starting work. Ordinary
runtime requests do not re-query roles or call providers; privileges themselves
remain the enforcement boundary. Re-run checks after any privilege change.

## Action

1. Use a new disposable database to rehearse. Install required extensions with
   the database administrator and apply the append-only migrations through
   0188 using `cargo sqlx migrate run --source crates/app/migrations --no-dotenv`
   with the explicit bootstrap administrator credential. This first adoption
   is an explicit administrator operation, separate from normal production
   `deepref-server migrate`. Never point a test at an existing user database.
2. Through the selected credential mechanism, provision **three distinct LOGIN
   identities**, with INHERIT and no SUPERUSER, CREATEDB, CREATEROLE, REPLICATION,
   BYPASSRLS or unrelated memberships. This repository does not generate, reset
   or store their passwords. Hosting and the secret manager remain undecided.
3. Stop processes using the old owner credential and verify that release's
   actual drain. Diagnostic heartbeats do not prove that. With the database
   administrator credential, execute:

   ```bash
   deepref-server database-roles provision --runtime-login <runtime_login> --migration-login <migration_login> --maintenance-login <maintenance_login>
   ```

   Activation is transactional and uses a bounded lock/statement budget. It
   adopts application objects into the NOLOGIN migration owner, preserving
   extension ownership; removes PUBLIC/direct-login application grants; assigns
   the three fixed NOLOGIN capability roles; and activates the catalog contract.
   Unsafe pre-existing capability roles are refused rather than modified. It
   grants CONNECT and ordinary application CRUD to runtime, with audit/proof
   append access and schema/migration/contract read access only. Maintenance has
   only the retention time/event columns needed for bounded counts, activation
   read access and the bounded retention function. It cannot read sealed session
   payloads/hashes, audit metadata or fulfillment address digests. Runtime cannot
   assume either other capability. These roles supplement the application PDP;
   they do not establish tenant RLS or protect against a trusted migration owner.

4. Verify all three credentials. Deploy API/worker with runtime only; deploy the
   once-per-release migration runner with the distinct migration LOGIN. Revoke
   and rotate the retired owner credential through the selected secret system.
   No source check proves a deployed credential was rotated or an old process
   stopped. Each new migration must preserve grants on protected tables and
   explicitly grant any new application function it needs; default function
   execution is not public. Provisioning clears direct LOGIN/PUBLIC column ACLs
   as well as relation ACLs; checks detect later column-level privilege drift.
   Repeated activation preserves narrower table/function
   grants declared after first adoption. New maintenance-only tables present at
   first adoption need explicit protected-table policy before activation; do not
   assume ordinary runtime default CRUD is appropriate for control tables.
5. Schedule `deepref-server audit purge` using the maintenance credential. The
   command reports `remaining_eligible`; repeat later while that is true. A dry
   run is read-only. Each real call caps audit batches at 32 and batch size at
   1000, plus one bounded session/fulfilled-proof batch. A three-second statement
   deadline and 250ms lock deadline refuse contention; do not disable them to
   force a purge. Locked rows skipped by the bounded function remain eligible.

Ordinary/security audit expires after six calendar months; deletion-completed
proof and fulfilled-deletion digests after five years; sessions thirty days
past expiry/revocation. Deletion and its `audit.purged` summary commit in one
transaction. The fixed SECURITY DEFINER function uses `search_path=pg_catalog`
and qualified tables. Runtime custom GUCs cannot authorize deletion/redaction:
the trigger also checks the trusted definer owner and row retention/state.
Maintenance cannot directly mutate the audit, change the function, or reset
retention. The migration owner remains trusted DDL authority.

## Evidence

Retain role-check results, migration/release identity, activation timestamp,
credential-rotation/drain evidence and bounded purge counts/remaining flag. Local
real LOGIN tests cover normal DML, distinct migration ownership, direct
UPDATE/DELETE/TRUNCATE/DDL/role/session-replication bypass refusals, an accidental
DELETE grant plus forged GUC, removal of preexisting direct column grants,
column-level mutation/read drift refusal, denied sealed-session/audit-payload
reads, fixed function search path, bounded maintenance
purge and preserved young/five-year proof rows. Real fixture roles and containers
are task-owned and removed; no production provisioning is performed by these
tests. Production activation/rotation and scheduler execution remain external
acceptance until evidenced on the chosen host.
