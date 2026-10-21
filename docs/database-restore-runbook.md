# Database restore runbook

Operating procedures for PostgreSQL point-in-time recovery (PITR) drills,
disaster recovery restores, and production egress containment.

## Baseline parameters

- **Backup architecture:** Continuous PostgreSQL WAL archiving paired with
  periodic base backups. Encrypted at rest, hosted on a private network only,
  and protected against accidental deletion in production.
- **Target metrics:** 35-day backup retention, Recovery Point Objective (RPO)
  no more than 5 minutes, Recovery Time Objective (RTO) no more than 60 minutes.
- **Compliance:** Backups constitute personal data. Retention windows must align
  with the LGPD retention policy established in the handoff 00 business gate.
- **Drill frequency:** Executed before production launch, at least quarterly,
  and prior to applying any contract-phase schema migration.

---

## 1. Routine restore drill execution

### Symptom

Scheduled quarterly restore drill, launch readiness verification, or an upcoming
contract migration requiring recovery validation.

### Query

Capture the live production database's latest restorable timestamp:

```sql
SELECT pg_last_wal_replay_lsn(), now() AS current_time;
```

Capture the corresponding object storage bucket inventory timestamp for the
matching time window.

### Action

Execute the drill using the following mandatory six-step sequence:

1. **Capture timestamps:** Record the source database's latest restorable time
   and the object-store inventory time.
2. **Restore PITR instance:** Restore PostgreSQL to a new, isolated database
   instance at the approved timestamp. Never overwrite or restore into the
   source production database.
3. **Resolve object versions:** Resolve object store versions as of the exact
   same timestamp into a quarantined, read-only bucket or prefix.
4. **Execute reconciliation:** Run the read-only reconciliation command against
   the restored database instance using a read-only PostgreSQL role.
5. **Isolate and test candidate workers:**
   - Configure the restored environment with isolated credentials and mock
     endpoints for all third-party services (WorkOS, Polar, AI providers).
   - Point object store configuration exclusively at the quarantined read-only
     bucket or prefix.
   - Enforce network-level egress filtering: block all outbound production
     egress from the drill network.
   - Quarantine all restored `queued` and `running` jobs in the database after
     step 4 has finished reading them, ensuring candidate workers cannot claim
     real customer work.
   - Start a candidate API and worker replica.
   - Enqueue controlled fixture test jobs and verify that `/health/ready` returns
     ready and the fixture jobs complete cleanly. Production jobs must never drain
     in the drill.
6. **Record metrics and teardown:** Record the measured RPO, RTO, reconciliation
   mismatch counts, and test output. Once documented, terminate and delete the
   restored instance and quarantined storage prefix.

### Evidence

Documented drill report confirming:

- Measured RPO <= 5 minutes.
- Measured RTO <= 60 minutes.
- Zero unexplained reconciliation mismatches between database rows and storage objects.
- All candidate worker executions drained only synthetic fixture jobs.
- The restored database and quarantined bucket resources were destroyed.

---

## 2. Production point-in-time recovery (disaster recovery)

### Symptom

Catastrophic database corruption, data loss, or accidental truncation requires
recovering production state to a point prior to the incident.

### Query

Identify the precise incident onset timestamp ($T_{incident}$):

```sql
SELECT query, query_start, state
FROM pg_stat_activity
WHERE state != 'idle';
```

Check the latest available base backup and continuous WAL archive segment:

```sql
SELECT last_archived_wal, last_archived_time
FROM pg_stat_archiver;
```

### Action

1. Establish the target recovery timestamp ($T_{target} = T_{incident} - 1\text{ minute}$).
2. Stop all production API and worker replicas immediately to freeze incoming mutations.
3. Provision a new production database instance from the base backup.
4. Apply WAL replay up to the designated target timestamp.
5. Run `deepref-server migrate` if the restored target requires schema alignment.
6. Align object storage versioning by restoring noncurrent versions to match
   the database snapshot timestamp.
7. Run the read-only reconciliation check to audit state before resuming traffic.
8. Repoint application API and worker pools to the restored database and launch
   services.

### Evidence

The restored database boots successfully. `/health/ready` returns HTTP 200 on all
replicas. Queries confirm data integrity prior to the corruption timestamp.

---

## 3. Candidate worker isolation and egress containment

### Symptom

During a restore drill or disaster recovery dry run, candidate workers risk
contacting external production APIs, charging customer accounts, or attempting
to claim production work.

### Query

Inspect candidate worker environment variables for production secrets:

```bash
env | grep -E "WORKOS|POLAR|DOCUMENT_STORAGE|OPENAI|ANTHROPIC"
```

Check egress packet counters from the isolated drill subnet:

```bash
iptables -L OUTPUT -v -n
```

Query the restored jobs table for non-quarantined customer jobs:

```sql
SELECT count(*)
FROM jobs
WHERE state IN ('queued', 'running') AND kind NOT LIKE 'fixture_%';
```

### Action

1. Ensure the drill network environment has no route to the live production
   database or production network subnets.
2. Note that database lease fencing fences only workers connected to that same
   database instance; it cannot fence live production workers. Therefore,
   isolation must be enforced at the credential, endpoint, and network layers.
3. Apply egress firewall rules blocking all outbound public IP traffic except
   approved local mock servers.
4. Update worker configuration to supply isolated or dummy API keys for WorkOS,
   Polar, and AI providers.
5. Ensure step 5 quarantine updates all restored `queued` and `running` jobs to a
   quarantined state (`state = 'drill_quarantined'`) before worker processes
   launch.

### Evidence

Firewall logs show zero egress traffic to production third-party IPs. Restored
workers process only synthetic drill fixtures. Live production systems experience
zero duplicate job claims or unauthorized external calls.

---

## 4. Recovery Point Objective (RPO) or Recovery Time Objective (RTO) breach

### Symptom

A restore drill or test recovery exceeds the 60-minute RTO ceiling or detects
unarchived WAL lag exceeding the 5-minute RPO threshold.

### Query

Compute the achieved RTO from drill execution timestamps:

```text
RTO = (Timestamp when /health/ready returned 200) - (Timestamp restore was initiated)
```

Compute the achieved RPO from WAL archive lag:

```sql
SELECT EXTRACT(EPOCH FROM (now() - last_archived_time)) AS wal_lag_seconds
FROM pg_stat_archiver;
```

### Action

1. If RPO exceeds 5 minutes, inspect PostgreSQL WAL archiver configuration.
   Decrease `archive_timeout` to force frequent WAL segment rotation. Verify
   network throughput from the database to the WAL archive destination.
2. If RTO exceeds 60 minutes, profile the restoration pipeline:
   - Measure snapshot download and storage provisioning time.
   - Measure WAL replay throughput. Enable parallel WAL replay if supported by
     the PostgreSQL platform.
3. Remediate platform bottlenecks and repeat the drill until target thresholds
   are achieved.

### Evidence

Subsequent restore drill log records achieved RTO <= 60 minutes and RPO <= 5
minutes.

---

## 5. LGPD backup data retention compliance

### Symptom

Personal data deleted by users in accordance with privacy requests remains in
database backups beyond the approved retention period.

### Query

Inspect backup snapshot catalog ages against the governing LGPD retention
policy window:

```bash
aws s3 ls s3://<backup-bucket>/wal/ --human-readable
```

### Action

1. Review the data retention period mandated by the LGPD policy from the 00
   business gate.
2. Configure automated backup lifecycle rules on the backup storage bucket to
   expire base backups and WAL segments older than the approved retention window
   (e.g., 35 days).
3. Ensure deletion protection does not prevent automated expiration of backups
   exceeding the retention schedule.

### Evidence

Backup storage bucket policies enforce lifecycle expiration. Backup inventory
confirms zero retained WAL segments or snapshots older than the mandated policy
window.
