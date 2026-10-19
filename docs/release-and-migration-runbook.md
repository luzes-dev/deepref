# Release and migration runbook

Operating procedures for releases, schema migrations, rolling deploys, and
rollbacks.

## Deployment principles

- Deployments run version N and version N-1 binaries concurrently against the
  same database schema. Every release must remain compatible across the
  overlap window in both directions.
- Database migrations always expand first, then contract in a subsequent release.
- Rollbacks redeploy N-1 container images against the N schema. Down-migrations
  are never executed.

### Expand and contract phases

```text
expand     Add nullable columns, new tables, new enum values, new indexes.
backfill   Batched and idempotent data backfills outside the request path.
dual       Writers write both old and new shapes; readers read the old shape.
switch     Readers cut over to the new shape in a subsequent release.
contract   Drop, rename, add NOT NULL, narrow a type, or remove an enum value.
           Only run after no deployed image reads the old shape, including
           scheduled jobs and worker pools.
```

- Every migration header must declare its phase: `-- phase: expand` or
  `-- phase: contract`.
- A contract migration must declare the retired release that is out of service.
- If a migration changes a job kind's payload or state set, it is contract-sensitive:
  N-1 workers must safely ignore or park the new state.
- Large index builds must use non-transactional migrations (`-- no-transaction`).
- API and worker deploy together for config-dependent startup writes.
  `ensure_default_model_routes` must yield identical results under N and N-1.

---

## 1. Standard pre-deploy migration execution

### Symptom

A new release is ready to deploy. The database schema must be updated before any
version N container replica is launched.

### Query

Inspect the current applied database schema version:

```sql
SELECT version, description, installed_on, success
FROM _sqlx_migrations
ORDER BY version DESC
LIMIT 5;
```

Check the active API readiness payload:

```bash
curl -fsS http://<api-endpoint>:8080/health/ready
```

### Action

1. Verify the release migration files in `crates/app/migrations` declare their
   appropriate `-- phase: expand` annotations.
2. Execute the migration runner once against the target database:
   ```bash
   deepref-server migrate
   ```
3. Do not start version N replicas until `deepref-server migrate` exits with code 0.
4. Active N-1 replicas will detect the newer schema; verify that `/health/ready`
   remains healthy (an N-1 binary logs a warning on a newer schema but does not
   fail readiness).

### Evidence

The migration command completes successfully. Querying `_sqlx_migrations` shows
the latest migration version applied with `success = true`. Existing N-1 API
replicas continue returning HTTP 200 on `/health/ready`.

---

## 2. Rolling replica rollout and drain

### Symptom

Database migration has succeeded; worker and API container pools must be
transitioned from version N-1 to version N.

### Query

Check rolling deployment progress in the target platform, and inspect replica
readiness:

```bash
curl -fsS http://<replica-ip>:8080/health/ready
```

Check worker heartbeats in the dependency health endpoint:

```bash
curl -fsS http://<api-endpoint>:8080/health/dependencies
```

### Action

1. Roll worker replicas to version N first, followed by API replicas.
2. Allow incoming SIGTERM signals to initiate graceful replica draining within
   the platform grace period.
3. API replicas must close idle keep-alive connections and terminate active
   assistant Server-Sent Events (SSE) connections cleanly so clients reconnect
   to version N replicas.
4. Worker replicas must complete in-flight tasks or hold leases until graceful
   shutdown.
5. Verify that version N replicas report `schema_version` equal to
   `required_schema_version`.

### Evidence

All API and worker replicas are running image version N. `/health/ready` returns
HTTP 200 with matching schema versions. `/health/dependencies` reports the
expected count of `live_workers`.

---

## 3. Replica schema mismatch during rollout

### Symptom

A newly started version N API replica returns HTTP 503 on `/health/ready` with
`schema_version` lower than `required_schema_version`, or an N-1 worker crashes
due to an unhandled job payload state.

### Query

Fetch the failing replica's readiness state:

```bash
curl -is http://<replica-ip>:8080/health/ready
```

Inspect worker error logs for job deserialization failures:

```text
grep -E "failed to deserialize job payload|unknown job kind|unhandled state" worker.log
```

### Action

1. If `/health/ready` returns 503, verify if `deepref-server migrate` was skipped
   or failed. Run `deepref-server migrate` once to apply pending migrations.
2. If an N-1 worker crashes on an unhandled job state introduced by version N,
   the release violated contract-sensitivity rules.
3. Pause rollout immediately. Requeue parked jobs and execute the rollback
   procedure if N-1 workers cannot proceed.

### Evidence

`/health/ready` on version N replicas returns HTTP 200 with
`"status":"ready"`. Worker error logs show zero deserialization or job-state
crashes.

---

## 4. Release rollback to N-1

### Symptom

Version N exhibits a fatal application error, elevated error rates, or data
integrity issues post-rollout. The release must be aborted.

### Query

Check error rate metrics on API and worker services:

```text
sum(rate(http_requests_total{status=~"5.."}[5m]))
```

Inspect recent failed jobs in the worker subsystem:

```sql
SELECT id, kind, state, attempts, error
FROM jobs
WHERE state = 'dead' AND updated_at > now() - INTERVAL '30 minutes';
```

### Action

1. Do NOT execute database down-migrations. Down-migrations destroy data and
   invalidate expand/contract guarantees.
2. Redeploy the version N-1 container images for both API and worker pools.
3. Because version N migrations followed expand rules (nullable columns, additive
   tables, dual-writing), version N-1 binaries continue functioning against
   the version N schema.
4. Confirm N-1 replicas start and report healthy readiness (logging a warning
   that the schema is newer than the binary).

### Evidence

All active replicas run version N-1 images. `/health/ready` returns HTTP 200
on all instances. 5xx request rates return to nominal baseline.

---

## 5. Post-deploy calibration bundle recording

### Symptom

After applying schema calibration changes (e.g. migration 0056), automated
screening runs are refused admission with
`CalibrationRefusal::IncompatibleIdentityScheme`.

### Query

Check stored calibration bundles for active definitions:

```sql
SELECT definition_id, stage_id, identity_scheme, status, created_at
FROM review_calibration_bundles
ORDER BY created_at DESC
LIMIT 10;
```

### Action

1. Note that no public HTTP endpoint or CLI records calibration bundles.
2. As documented in post-deploy operator actions, record one new scheme-3,
   `passing` calibration bundle per definition and stage intended to admit
   automation.
3. Extract bundle identities using the preview identity generators and insert
   the records directly into `review_calibration_bundles`.
4. Do not craft raw bundle identities by hand.

### Evidence

Querying `review_calibration_bundles` returns active rows with
`identity_scheme = 3` and `status = 'passing'`. Automated review scheduling
resumes admitting runs without scheme refusal errors.

---

## 6. Contract-phase migration execution

### Symptom

Old columns, tables, or enum values must be pruned from the database schema
after all readers and writers have switched away in preceding releases.

### Query

Verify that no version N-1 binaries or outdated worker pools remain active:

```bash
curl -fsS http://<api-endpoint>:8080/health/dependencies
```

Inspect application telemetry to ensure zero read or write operations target the
columns or tables slated for removal.

### Action

1. Ensure the migration file carries `-- phase: contract` and explicitly names
   the out-of-service release version.
2. Run `deepref-server migrate` once against the database.
3. If table locks are required on large tables, execute during an off-peak traffic
   window or use non-transactional DDL if supported.

### Evidence

`deepref-server migrate` exits 0. `_sqlx_migrations` records the applied
contract migration. All API and worker replicas continue reporting HTTP 200
on `/health/ready`.

---

## 7. Declared stop-the-world maintenance cutover

### Symptom

An exceptional, non-expandable migration (such as historical cutovers 0080/0081)
requires exclusive database access and cannot tolerate concurrent replica
traffic.

### Query

Verify that scheduled maintenance has been announced and user access windows are
closed. Check active connection counts:

```sql
SELECT count(*), application_name, state
FROM pg_stat_activity
GROUP BY application_name, state;
```

### Action

1. Declare a formal maintenance window with approval from the deployment owner.
2. Scale API and worker replicas to 0 instances.
3. Wait for all active connections and in-flight transactions to drain.
4. Execute `deepref-server migrate` once.
5. Scale worker replicas to version N, followed by API replicas.
6. Verify service health and revoke the maintenance window block.

### Evidence

All migrations complete successfully. Scaled-up version N replicas report
ready on `/health/ready`. Active database connections resume normal operating
levels.
