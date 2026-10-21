# Object store reconciliation runbook

Operating procedures for reconciling PostgreSQL metadata against S3-compatible
object storage, auditing blob integrity, and resolving orphan or missing data.

## Storage and retention baseline

- **Bucket versioning:** Bucket versioning must remain permanently enabled.
- **Noncurrent version retention:** Noncurrent version retention in object
  storage must be at least the PostgreSQL point-in-time recovery (PITR) window
  (35 days) plus a safety margin. A restored database must never reference an
  object version that object storage has expired.
- **Application-driven deletions:** Deletions are executed exclusively through
  cleanup jobs (after handoff 07 lands). Bucket lifecycle rules alone must never
  delete application data.
- **Tracked objects:** Object keys currently tracked in metadata include
  `documents.object_key` and `workflow_files.content` (until handoff 07 moves
  workflow files out of `bytea`).
- **Reconciliation tool contract:** Reconciliation is executed as a read-only
  command via `tools/xtask` or a documented standalone script. It reports all
  anomalies and **never deletes data**.

---

## 1. Missing object store blob for database metadata row

### Symptom

The reconciliation tool reports database rows referencing an `object_key` or
version that does not exist in the object storage bucket, or an API client
receives an object-not-found error when fetching a registered document.

### Query

Run the read-only reconciliation check for missing objects:

```bash
cargo xtask reconcile-storage --check-missing
```

Or query the database for suspect document keys:

```sql
SELECT id, project_id, object_key, created_at
FROM documents
WHERE object_key = '<missing-key>';
```

Verify version availability directly in object storage:

```bash
aws s3api list-object-versions --bucket <bucket-name> --prefix <missing-key>
```

### Action

1. Determine if the missing object was caused by an incomplete upload where the
   database transaction committed despite an S3 failure. (Handoff 12 requires
   uploads to fail without committing metadata rows).
2. Inspect the S3 version catalog. If a delete marker was mistakenly placed on
   the object, remove the delete marker to restore the current version.
3. If the noncurrent version was prematurely purged by an aggressive bucket
   lifecycle rule, restore the object from external backup or request client
   re-upload.
4. Quarantine affected metadata rows if recovery is impossible. Do not delete
   authoritative records manually.

### Evidence

Reconciliation rerun confirms 0 missing blobs for all active database metadata
rows.

---

## 2. Orphaned object store blobs without database metadata

### Symptom

Object storage contains blobs with no corresponding pointer in `documents`
(or `workflow_files` after handoff 07 lands).

### Query

Generate an inventory comparison report using the read-only reconciliation tool:

```bash
cargo xtask reconcile-storage --check-orphans --inventory-csv s3-inventory.csv
```

### Action

1. Verify whether orphans stem from aborted multipart uploads or canceled user
   creations where binary upload completed before database insertion.
2. Ensure reconciliation does not delete the orphaned blobs.
3. Pass the generated orphan manifest to the asynchronous cleanup worker pool
   (after handoff 07 lands) to verify grace periods (minimum 24 hours) before
   scheduling background removal.
4. Verify S3 multipart upload lifecycle policies abort uncompleted uploads
   older than 7 days.

### Evidence

Orphan count matches expected in-flight buffer. Scheduled cleanup runs process
and clear unreferenced temporary uploads.

---

## 3. Stored content hash mismatch on sampled objects (integrity failure)

### Symptom

Reconciliation integrity audit detects a mismatch between the stored SHA-256
hash in the database metadata and the SHA-256 recomputed over the object payload
(handoff 07 integrity).

### Query

Run the read-only hash audit over a representative sample:

```bash
cargo xtask reconcile-storage --audit-hashes --sample-rate 0.05
```

Fetch metadata and hash details for the damaged record:

```sql
SELECT id, object_key, content_hash, byte_size, updated_at
FROM documents
WHERE id = '<document-id>';
```

### Action

1. Isolate the affected document immediately to prevent serving corrupted
   evidence.
2. Check S3 object version history to determine if an uncoordinated overwrite
   (PUT) occurred under the same key without updating the database hash.
3. Compare the current S3 object version timestamp against the database
   `updated_at` timestamp.
4. If corruption occurred at rest, restore the object from the preceding clean
   version in object storage versioning.
5. If the mismatch represents an unversioned overwrite, recompute the hash from
   the authoritative source file or re-ingest evidence.

### Evidence

Recomputed hash matches the database metadata record. Re-audit across the
document collection reports zero hash discrepancies.

---

## 4. Unreclaimable running jobs with expired leases or unprocessed inbox rows

### Symptom

Reconciliation reports jobs lingering in `running` status whose leases expired
and active workers are unable to reclaim, or webhook inbox rows that have been
received but never processed (after handoff 04a lands).

### Query

Check for stalled running jobs with expired leases:

```sql
SELECT id, kind, state, attempts, lease_expires_at, worker_id
FROM jobs
WHERE state = 'running' AND lease_expires_at < now();
```

Check for unprocessed webhook inbox rows (after handoff 04a lands):

```sql
SELECT id, source, event_type, received_at, attempts
FROM webhook_inbox
WHERE status = 'unprocessed' AND received_at < now() - INTERVAL '15 minutes';
```

### Action

1. For expired jobs: verify if `recover_expired_jobs` is functioning in the
   worker pool. If workers are stalled, restart worker replicas to resume the
   requeue cycle.
2. For jobs exceeding `max_attempts`: allow the worker lifecycle to transition
   them to `dead`. Inspect failure payloads to identify poison pills.
3. For unprocessed webhook inbox rows (after handoff 04a lands): inspect
   webhook consumer logs for parser errors or database transaction deadlocks.
   Trigger consumer reprocessing.

### Evidence

Stalled running job count returns to 0. Unprocessed webhook inbox backlog clears
to 0 (after handoff 04a lands).

---

## 5. Expired unsettled usage reservations

### Symptom

Reconciliation detects usage reservations that remain in pending status past
their expiration timestamp without settlement (after handoff 09 lands).

### Query

Query expired, unsettled usage reservations:

```sql
SELECT id, customer_id, meter_type, amount, status, expires_at
FROM usage_reservations
WHERE status = 'pending' AND expires_at < now();
```

### Action

1. Verify if the Polar billing exporter or background settlement task is running
   (after handoff 09 lands).
2. Trigger the reservation cleanup job to release reserved units back to customer
   balances.
3. If reservations failed to settle due to a Polar API outage, verify that usage
   records are held with immutable identities until the provider recovers.

### Evidence

Query returns zero reservations remaining in `pending` past `expires_at`.

---

## 6. Object storage noncurrent retention window expiration breach

### Symptom

Object storage bucket lifecycle configuration is discovered to have a noncurrent
version retention duration shorter than the PostgreSQL PITR window (35 days plus
margin).

### Query

Fetch the active S3 bucket lifecycle configuration:

```bash
aws s3api get-bucket-lifecycle-configuration --bucket <bucket-name>
```

### Action

1. Immediately update the bucket lifecycle rule governing `NoncurrentVersionExpiration`.
2. Set `NoncurrentDays` to at least 45 days (35-day PostgreSQL PITR window plus
   a 10-day operator margin).
3. Ensure no rule automatically purges delete markers while underlying versions
   are still retained.

### Evidence

The bucket lifecycle configuration reflects `NoncurrentDays >= 45`.
