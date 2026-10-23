# Database restore runbook

Restore operations require the deployment owner and platform procedure recorded
in [ADR 0008](adr/0008-production-deployment-ownership-and-target.md). Owner,
hosting target, secret infrastructure and metrics remain pending. This runbook
does not select a provider or claim that backups/PITR have been provisioned.

The decided backup retention is a rolling 35 days under
[BUSINESS_DECISIONS](saas-program/BUSINESS_DECISIONS.md). Proposed RPO <= 5 minutes,
RTO <= 60 minutes and quarterly drills still require owner confirmation and
measurement. Never report achieved RPO/RTO from this document or a successful
local reconciliation alone.

## Isolated restore comparison

### Symptom

A launch rehearsal, quarterly drill, incident investigation or contract-phase
migration needs evidence that restored metadata and retained objects agree.

### Query

Obtain the latest restorable timestamp and backup identity from the platform's
backup/WAL catalog. `pg_last_wal_replay_lsn()` on a primary is not a backup
catalog; `pg_stat_archiver.last_archived_time` alone does not prove recoverable
RPO. Record the approved recovery target, source event/commit reference,
restored database identity and object inventory snapshot separately.

Run the actual [read-only reconciliation command](object-store-reconciliation-runbook.md#inputs-and-isolation)
with an explicitly configured drill URL and quarantined offline objects:

```bash
cargo xtask reconcile-storage --config /absolute/path/drill.json > /absolute/path/report.json
```

### Action

1. Restore to a **new isolated database** using the selected platform procedure.
   Keep all application workers/API processes stopped. Never use the live
   development or production database as the drill fixture.
2. Independently resolve object versions for the same recovery timestamp and
   materialize them in an immutable, read-only quarantine directory. Capture
   inventory export time, selection method and completeness. Cloud PITR and
   historical object-version resolution are prerequisites supplied by the
   platform/operator; the local tool implements neither.
3. Run reconciliation with a read-only database role. It also requests one
   repeatable-read/read-only transaction, bounded reads and timeouts itself.
   It never migrates or repairs the restored schema. Unsupported schema/read
   failures return incomplete evidence, not success.
4. Preserve JSON, exit code, target/cutoff, scan coverage and explanations for
   every mismatch. Exit 2 (partial/failed/budget-limited) cannot be accepted as
   a clean comparison. Exit 0 is only `no_anomalies_in_completed_checks`;
   `full_restore_verified` remains false, including for key-only version evidence
   and bounded content-hash samples.
5. Before any separate candidate-worker test, establish the isolation and
   customer-work containment prerequisites below. Do not start workers until
   they are demonstrably satisfied. This slice provides no mutation/quarantine
   command or worker-isolation harness.
6. When a separately approved drill includes startup/fixture tests, record the
   measured recovery point and elapsed time with their actual endpoints and
   evidence. Record teardown of only the drill's resources under the platform's
   approved procedure. Reconciliation performs no teardown or deletion.

### Evidence

A report identifies the actual restore target, backup/catalog evidence,
operator-attested inventory selection, database observation snapshot, scan
limits/completeness, anomalies and sample coverage. A complete production
acceptance additionally needs the measured, isolated runtime drill and all
remaining [handoff 12 gates](saas-program/12_OPERATIONS_ACCEPTANCE.md).

## Candidate worker isolation and production egress containment

### Symptom

A candidate worker could claim restored customer work or contact live identity,
billing, AI or other external services.

### Query

Inspect configuration **names/presence and approved endpoint identities** using
the selected secret manager's redacted tooling. Do not print environment values
or production secrets into drill logs. Review network-policy evidence and the
current active work set on the restored database:

```sql
SELECT state, kind, count(*)
FROM jobs
WHERE state IN ('queued', 'capability_wait', 'running')
GROUP BY state, kind
ORDER BY state, kind;
```

### Action

The drill network must have no route to the live database or production
provider endpoints. Supply isolated credentials/mock endpoints for WorkOS,
the eventual billing provider, AI providers and every other outbound target;
point object reads exclusively at quarantined objects. Database lease fencing
only applies within that database and cannot fence live production workers.

A separately reviewed containment procedure/harness must prevent **all restored
customer work**, including capability-wait, inbox/outbox and scheduled recovery,
from being claimed before candidate processes start. No supported
`drill_quarantined` job state exists. Do not invent one or run an ad-hoc state
UPDATE. A safe fixture-only startup harness remains a prerequisite outside this
read-only slice; while absent, keep workers stopped and report this part of the
drill as pending.

### Evidence

The separate exercise proves zero customer claims and production egress, plus
successful controlled fixtures. A reconciliation report cannot prove those
properties.

## Disaster recovery promotion

### Symptom

An approved incident response requires promoting a recovered copy to production.

### Query

Use the incident timeline, authoritative backup/WAL catalog and object-version
selection evidence to choose a recovery point. Reconcile the isolated copy
first and resolve every unexplained anomaly or incomplete check.

### Action

Follow the selected platform owner's approved cutover procedure, including
stopping live mutations, schema/release compatibility checks, aligned object
versions, application endpoints, credentials and readiness validation. Never
run down-migrations. Any necessary schema migration is a separate reviewed
operation, not part of reconciliation.

Before reopening access, follow [Security: Restore step](security.md#restore-step):
retain/re-import the protected `fulfilled_account_deletions` log, supply the
original digest-key versions, and execute the authorized
`deepref-server accounts complete-deletions` replay against the promotion
candidate. This intentionally mutating privacy operation is separate from the
read-only comparison. Restored sessions and user content must not become
accessible before fulfilled deletions are reapplied.

### Evidence

The incident record contains approved cutover, privacy replay, readiness,
integrity and runtime-isolation evidence, plus measured recovery values. The
read-only report contributes comparison evidence only.

## RPO/RTO breach and retention

### Symptom

Measured recovery exceeds the signed objectives, backup coverage is incomplete,
or deleted personal data survives beyond the 35-day rolling backup window.

### Query

Compare actual source commit/event timestamps and the verified recovery point
for RPO. Measure RTO from the recorded incident/drill start to the agreed service
recovery condition. A WAL archive activity timestamp is only one diagnostic,
not a substitute for either measure. Inspect the selected platform's backup and
object-version catalogs against the agreed retention policy.

### Action

Have the owner investigate backup coverage, replay/provisioning time, object
selection and recovery validation bottlenecks. Confirm encrypted backups,
private access, deletion protection and lifecycle expiration are compatible
with the decided retention. Repeat the measured drill after remediation;
provider-specific configuration commands remain pending platform selection.

### Evidence

Retained catalog/configuration records and a subsequent measured drill support
the objectives. Record unmet or unmeasured objectives explicitly.
