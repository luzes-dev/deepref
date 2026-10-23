# Object store reconciliation runbook

`cargo xtask reconcile-storage --config /absolute/path/drill.json` collects
read-only evidence from an isolated restored PostgreSQL database and an offline,
materialized object inventory. It makes no cloud calls, restores no versions,
changes no rows or objects, and starts no API or worker. This is one scoped part
of [operations acceptance](saas-program/12_OPERATIONS_ACCEPTANCE.md), not a
production restore drill or proof of RPO/RTO.

## Inputs and isolation

### Symptom

A restored copy needs comparison with objects selected for the same recovery
timestamp, or an operator needs a bounded integrity/backlog report.

### Query

Set `DEEPREF_RESTORE_DATABASE_URL` privately to the isolated drill database using
your credential injection mechanism. Use a read-only role. There is no fallback
to `DATABASE_URL`, `.env`, application settings, development storage or a cloud
bucket. The command also enforces read-only transactions when the supplied role
has more privileges. The operator must verify the endpoint is the drill copy;
a URL name alone cannot establish this.

Create an explicit JSON config. Replace all paths and timestamps with the drill
values; these limits are examples, not defaults:

```json
{
  "database_url_env": "DEEPREF_RESTORE_DATABASE_URL",
  "quarantine_root": "/absolute/path/drill/objects",
  "inventory": "/absolute/path/drill/inventory.json",
  "restore_target": "2026-10-22T12:00:00Z",
  "stale_before": "2026-10-22T11:45:00Z",
  "limits": {
    "rows_per_table": 10000,
    "page_rows": 200,
    "inventory_bytes": 8388608,
    "inventory_rows": 10000,
    "hash_samples": 100,
    "hash_object_bytes": 16777216,
    "hash_total_bytes": 268435456,
    "statement_timeout_ms": 10000,
    "overall_timeout_ms": 120000
  }
}
```

Accepted ceilings are 100,000 rows per table/inventory, 1,000 rows per page,
16 MiB of inventory JSON, 1,000 hash samples, 64 MiB per sampled object,
1 GiB of total hash bytes, 60 seconds per statement and 600 seconds overall.
Limits must be positive except `hash_samples`, which may be zero; size/existence
checks still cover every inventory/reference when hashing is disabled.

The inventory is a bounded JSON document, with one selected materialization per
exact key. This empty inventory is valid only when the complete selected set
really is empty:

```json
{
  "schema_version": 1,
  "snapshot_id": "drill-20261022",
  "captured_at": "2026-10-22T12:10:00Z",
  "restore_target": "2026-10-22T12:00:00Z",
  "selection": "materialized_as_of",
  "complete": true,
  "entries": []
}
```

Each entry has this shape:

```json
{
  "key": "documents/2f9f69b8-b809-4d36-a4bd-bdf2a55f7c41",
  "byte_size": 1234,
  "version_id": "optional-operator-selected-version-label"
}
```

The bytes must already exist at `quarantine_root/<key>`. `version_id` can be
omitted or null. Duplicate keys, including identical duplicates, fail the scan.
Absolute keys, empty/dot/dot-dot components, backslashes, percent encodings,
colons, control characters and symlink traversal are refused. Every component
of the existing root must be free of symlinks. Nonregular materializations are
refused. Keep the quarantine immutable, read-only and inaccessible to untrusted
writers for the entire scan; pathname checks do not prevent a local writer
racing a file lookup. The tool creates no directories or files.

`restore_target` must match between config and inventory. `captured_at` is the
inventory export time, at or after the target. `complete` and
`materialized_as_of` are **operator attestations**: the tool cannot verify the
external snapshot's authenticity or the provider's historical version
selection. The restored database's actual recovery point is also an operator
attestation. Current database references have no version ID; version labels
never prove historical version agreement. A key-only current-bucket listing
must not be described as a point-in-time object snapshot.

### Action

Run the command and retain both JSON and its exit status:

```bash
cargo xtask reconcile-storage --config /absolute/path/drill.json > /absolute/path/report.json
```

The command uses one `REPEATABLE READ READ ONLY` snapshot with statement, lock,
idle-transaction and overall timeouts. It reads bounded keyset pages, then rolls
back before reading object bodies. It never selects workflow file bodies from
PostgreSQL. Reports include the database observation time/snapshot ID, target,
cutoff, limits, counts and completeness of each scan. Errors use fixed public
codes and omit connection strings, raw database errors and credentials.

### Evidence

- Exit **0**: `no_anomalies_in_completed_checks`, within the declared scope.
- Exit **1**: completed checks found anomalies requiring review.
- Exit **2**: invalid config/inventory, refused path, failed read, timeout,
  exhausted budget or incomplete scan, even if anomalies were also found.

Always inspect `complete`, `failures`, per-table scans and object/hash coverage.
`full_restore_verified` is always false. An incomplete metadata scan produces
only unmatched inventory **candidates**, never confirmed orphans. A deliberately
bounded hash sample is not a full integrity scan; a selected sample exceeding
a byte budget causes exit 2 instead of silently choosing a smaller file.

## Missing objects, versions, size or sampled hash mismatches

### Symptom

The report identifies a reference absent from inventory, a missing
materialization (with a selected-version label where supplied), invalid
metadata, a size mismatch, or `sampled_hash_mismatch`.

### Query

The tool scans all document statuses and every nonnull `documents.object_key`,
including uploaded/failed rows. Object-key UUIDs need not equal document IDs.
Inspect actual metadata on the drill copy:

```sql
SELECT id, report_id, status, object_key, content_hash, byte_size, created_at
FROM documents
WHERE id = '<document-id>';

SELECT id, project_id, run_id, storage_key, content_sha256, byte_size,
       octet_length(content) AS inline_bytes
FROM workflow_files
WHERE id = '<workflow-file-id>';
```

Pure inline-only workflow rows have all three reference columns null and
existing inline bytes. They are compatibility content pending backfill, not
missing objects. Partially null reference metadata is invalid. Migration 0084
has a validated CHECK, but SQL null semantics permit some partial-null rows;
the tool checks them explicitly. Inline fallback never hides an anomaly in an
object-backed row. Every referenced/inventory materialization is checked for
existence, regular-file type and size independently of hash sampling.

### Action

Preserve evidence and verify inventory completeness, target alignment and the
operator's selected version against the backup catalog. Recover bytes through
a separately approved provider procedure, then regenerate the immutable
quarantine and rerun. This command cannot remove delete markers or select a
historical version. Do not alter metadata to make an integrity mismatch vanish.

### Evidence

A completed comparison explains every discrepancy, and reports the exact hash
sample checked. No claim of full-object integrity or historical version
agreement follows from zero sampled mismatches.

## Inventory orphans

### Symptom

`inventory_orphan` identifies an inventory key with no reference after both
metadata scans complete.

### Query

Use the report's exact key and snapshot information. Orphans can result from
aborted uploads, normal deletion, or a post-target/current inventory compared
with an older restored database. Confirm target/version alignment first.

### Action

Retain orphans in the drill. Never feed this report directly to a delete
command. Durable cleanup/grace-period handling remains a separate handoff 07
slice and must consider backups and in-flight work.

### Evidence

Each orphan is explained or retained for investigation. Reconciliation deletes
nothing.

## Expired job leases and integration backlogs

### Symptom

The report contains expired-running-job candidates, missing leases,
unknown-kind evidence, stale pending work or unprocessed inbox/outbox records.

### Query

These are current schema names, not a quarantine/update procedure:

```sql
SELECT id, kind, state, attempts, max_attempts, claim_seq, leased_until
FROM jobs
WHERE state = 'running' AND leased_until < transaction_timestamp();

SELECT id, processing_state, received_at, attempts, next_attempt_at,
       lease_expires_at
FROM integration_inbox_events
WHERE processing_state <> 'processed';

SELECT event_id, delivery_state, created_at, next_attempt_at, lease_expires_at
FROM outbox_events
WHERE delivery_state <> 'delivered';
```

### Action

Distinguish lease expiry at the restore target from expiry at database
observation. Job recovery uses strict `<`; inbox/outbox lease expiry uses `<=`.
A known expired job below `max_attempts` is a retry candidate; at the limit it
is a dead-letter candidate. Unknown kinds need inspection. None of these proves
that a worker is unable to recover it. Validate capable worker/pool availability,
claim fencing and isolation in a separate controlled drill before promotion.
Do not start workers on restored customer work merely to clear this report.

### Evidence

Record queue states, cutoff and recovery classification. The separate worker
exercise must show actual reclaimability and controlled egress; this read-only
command supplies no such runtime proof.

## Stale nonterminal usage reservations

### Symptom

Nonterminal reservations have `opened_at <= stale_before`.

### Query

```sql
SELECT r.id, r.state, r.opened_at, r.job_id, r.attempt_fence,
       r.recognized_cost_micros, r.observed_input_tokens,
       r.observed_output_tokens, r.observed_reasoning_tokens,
       r.reasoning_micros_per_million_tokens,
       j.claim_seq, j.state AS job_state, j.lease_owner, j.leased_until
FROM ai_usage_reservations r
LEFT JOIN jobs j ON j.id = r.job_id
WHERE r.state IN ('open', 'observed', 'unresolved')
  AND r.opened_at <= '<diagnostic-cutoff>'::timestamptz;
```

### Action

There is **no reservation `expires_at`**. Age is diagnostic, never permission to
void a hold or evidence of zero provider cost. The report separates complete
persisted token observations (including reasoning tokens when priced),
recognized liability, and the exact job fence evidence used by current
reconciliation: missing job, superseded attempt, released job, unfenced active
attempt, or request-owned reservation with no job fence. An expired lease alone
does not fence the attempt for settlement. Known observations and ambiguous
liabilities need the existing authorized recovery/operator process; this tool
settles, exports and voids nothing. Billing-provider choice remains pending.

### Evidence

Every stale reservation has reviewed observation, liability and fence evidence.
A nonterminal hold can legitimately remain while liability is unresolved.

## Version retention prerequisite

### Symptom

The backup catalog cannot supply object versions for the full recovery window.

### Query

Ask the selected platform's owner for the versioning, noncurrent-version and
backup lifecycle configuration and inventory coverage. No provider is selected
by this runbook or command.

### Action

Maintain the decided 35-day PostgreSQL backup window and object noncurrent
retention of at least that window plus an owner-approved margin. Application
cleanup must account for recoverability. Provisioning and retention validation
remain pending production work.

### Evidence

An owner-approved catalog and later measured drill demonstrate coverage. A
local reconciliation report does not validate cloud retention configuration.
