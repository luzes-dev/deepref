# SLO incident runbook

Operating procedures for alerting on and remediating Service Level Objective
(SLO) breaches across API, worker, platform, billing, and AI subsystems.

## Service Level Indicators and Owners

| SLI                                        | Source                                            | Target          | Window  | Owner          |
| ------------------------------------------ | ------------------------------------------------- | --------------- | ------- | -------------- |
| API availability (non-5xx ratio)           | API metrics                                       | >= 99.9%        | 30 days | API owner      |
| API latency p95 by route class (excl. SSE) | API metrics                                       | < 500 ms        | 30 days | API owner      |
| API replicas below required schema version | `/health/ready`                                   | 0 after rollout | 5 mins  | API owner      |
| Job queue age per workload pool            | `worker.oldest_age_seconds` (per pool after 05a)  | < 60 s          | 15 mins | Worker owner   |
| Dead jobs in 30-minute window              | `worker.recent_failed`                            | 0               | 30 mins | Worker owner   |
| Live workers against expected              | `worker.live_workers` (heartbeat 15s, window 60s) | >= expected     | 5 mins  | Worker owner   |
| Outbox backlog age                         | SQL (`ai_autonomy_outbox`; after 04a)             | < 30 s          | 15 mins | Platform owner |
| Inbox backlog age & webhook lag            | SQL (after 04a)                                   | < 60 s          | 15 mins | Billing owner  |
| Usage settlement backlog                   | SQL (after 09)                                    | < 300 s         | 1 hour  | AI owner       |
| Backup freshness and WAL archive lag       | Database archiver status                          | < 300 s (RPO)   | 15 mins | Platform owner |
| Object-store error rate and orphans        | Storage metrics & reconciliation                  | < 0.1% errors   | 1 hour  | Platform owner |

### Alerting rules

- Every SLO defines both a **ticket threshold** (approaching burn rate or mild
  degradation) and a **page threshold** (critical burn rate or outage).
- Every alert notification route must be validated in production with an active
  test alert; an alert without a tested notification delivery route is not
  evidence.
- **Privacy constraint:** Metric labels must never include workspace, project,
  user, or cost identifiers (per handoff 09 observability constraints).

---

## 1. API availability degradation (elevated 5xx ratio)

### Symptom

API non-5xx ratio falls below the agreed SLO threshold (e.g. < 99.9%). The API
owner is paged.

### Query

Check overall availability over 5-minute and 30-minute windows:

```text
sum(rate(http_requests_total{status!~"5.."}[5m]))
/ sum(rate(http_requests_total[5m]))
```

Inspect API error logs for 500 Internal Server Errors:

```bash
grep "HTTP 500" api.log | tail -n 20
```

### Action

1. Check `/health/ready` across all API replicas.
2. If errors correlate with database connection timeouts, inspect PostgreSQL
   active connection limits and connection pool settings.
3. If errors follow a new release deployment, follow the release runbook and
   redeploy N-1 images immediately.
4. Scale out API replicas if CPU or memory saturation is driving dropped requests.

### Evidence

Availability metric recovers above 99.9%. Error logs show 5xx rates returning
to 0.

---

## 2. API latency p95 threshold breach (non-SSE route classes)

### Symptom

P95 response latency exceeds threshold (e.g. > 500ms) on non-streaming HTTP
routes. The API owner receives an alert.

### Query

Query 95th percentile latency grouped by route class (excluding SSE streams):

```text
histogram_quantile(0.95, sum(rate(http_request_duration_seconds_bucket{route_class!="sse"}[5m])) by (le, route_class))
```

### Action

1. Identify the route class driving elevated latency (e.g. document indexing,
   screening queries).
2. Check database query performance for table scans, index contention, or lock
   delays:
   ```sql
   SELECT pid, age(clock_timestamp(), query_start), query
   FROM pg_stat_activity
   WHERE state != 'idle' AND age(clock_timestamp(), query_start) > INTERVAL '1 second';
   ```
3. Check for external API dependency delays if routes execute outbound calls.
4. Apply read replicas or query index optimizations as necessary.

### Evidence

P95 latency returns below 500ms across all non-SSE route classes.

---

## 3. API replicas running outdated schema version

### Symptom

Alert triggers: one or more API replicas report `schema_version` below
`required_schema_version` post-rollout. Owner: API owner.

### Query

Query the readiness state across all active API instances:

```bash
for ip in $(cat replica_ips.txt); do
  curl -s "http://${ip}:8080/health/ready" | jq '{ip: "'$ip'", schema: .schema_version, required: .required_schema_version}'
done
```

### Action

1. Determine if the deployment rollout stalled, leaving old N-1 replicas in
   service while traffic is routed to them.
2. Confirm if `deepref-server migrate` ran. If omitted, run the migration runner
   once.
3. Terminate stuck N-1 containers to force the orchestrator to replace them with
   version N replicas.

### Evidence

All running API pods return HTTP 200 with `schema_version == required_schema_version`.
The alert resolves.

---

## 4. Job queue age elevation

### Symptom

Oldest queued job age exceeds the maximum permissible wait time. Owner: Worker
owner.

### Query

Inspect `/health/dependencies` on an active API replica:

```bash
curl -fsS http://<api-endpoint>:8080/health/dependencies | jq '.worker.oldest_age_seconds'
```

Query the database directly:

```sql
SELECT max(EXTRACT(EPOCH FROM (now() - created_at))) AS oldest_queued_seconds
FROM jobs
WHERE state = 'queued';
```

_(Per workload pool after handoff 05a lands)._

### Action

1. Check if worker replicas are active and reporting heartbeats.
2. If `live_workers` is below expected, scale or restart the worker pool.
3. Check for long-running blocking jobs occupying worker execution threads.
4. Scale worker replica concurrency if incoming job arrival volume has spiked.

### Evidence

`worker.oldest_age_seconds` falls below the alert threshold (e.g. < 60s).

---

## 5. Dead jobs accumulation

### Symptom

The count of dead jobs (`recent_dead`) exceeds 0 within a 30-minute window.
Owner: Worker owner.

### Query

Check the recent dead job count via `/health/dependencies` or SQL:

```bash
curl -fsS http://<api-endpoint>:8080/health/dependencies | jq '.worker.recent_failed'
```

```sql
SELECT id, kind, attempts, error, updated_at
FROM jobs
WHERE state = 'dead' AND updated_at > now() - INTERVAL '30 minutes';
```

### Action

1. Inspect the stored `error` messages on the dead job records.
2. Categorize failure cause: upstream third-party failure, database transient
   lock, or deterministic code defect.
3. If failures were caused by a resolved transient outage, requeue the jobs for
   retry.
4. If failures represent an unhandled bug or schema incompatibility, file a bug,
   patch the worker code, and hotfix.

### Evidence

`worker.recent_failed` drops to 0. Unhandled exceptions disappear from worker
telemetry.

---

## 6. Live worker count below expected

### Symptom

The number of live workers reporting heartbeats falls below the expected replica
count. Owner: Worker owner.

### Query

Fetch live worker count from `/health/dependencies`:

```bash
curl -fsS http://<api-endpoint>:8080/health/dependencies | jq '.worker.live_workers'
```

Query the underlying worker heartbeat table (heartbeat interval 15s, 60s window):

```sql
SELECT count(DISTINCT worker_id) AS active_workers
FROM worker_heartbeats
WHERE last_seen > now() - INTERVAL '60 seconds';
```

### Action

1. Check worker container restart counts and exit codes in the orchestration
   target.
2. Review worker logs for out-of-memory (OOM) kills or startup panics.
3. Verify worker process connectivity to PostgreSQL.
4. Restart terminated worker pods and increase memory/CPU limits if OOM killed.

### Evidence

`worker.live_workers` equals or exceeds the provisioned worker replica target.

---

## 7. Outbox backlog age elevation (after handoff 04a lands)

### Symptom

Events queued in the transactional outbox (`ai_autonomy_outbox`, and general
outbox after handoff 04a lands) exceed age thresholds without relay. Owner:
Platform owner.

### Query

Check pending outbox rows:

```sql
SELECT count(*), max(EXTRACT(EPOCH FROM (now() - created_at))) AS max_lag_seconds
FROM ai_autonomy_outbox
WHERE status = 'pending';
```

### Action

1. Inspect the outbox relay task logs in the worker pool.
2. Check connectivity from the outbox relay to downstream message destinations
   (or NATS if T-NATS triggered).
3. If the relay process crashed, restart worker background tasks.

### Evidence

Max outbox age drops below 30 seconds; pending row count drains to 0.

---

## 8. Inbox backlog age and webhook processing lag (after handoff 04a lands)

### Symptom

Webhook events from WorkOS or Polar sit in the incoming inbox without processing.
Owner: Billing owner.

### Query

Query unprocessed webhook inbox rows (after handoff 04a lands):

```sql
SELECT source, count(*), max(EXTRACT(EPOCH FROM (now() - received_at))) AS max_lag
FROM webhook_inbox
WHERE status = 'unprocessed'
GROUP BY source;
```

### Action

1. Inspect the webhook inbox processor worker process.
2. Check for schema parsing errors or unexpected payloads from external webhook
   senders.
3. If processing deadlocked, restart the inbox consumer and trigger batch
   reprocessing.

### Evidence

Webhook inbox processing lag returns below 60 seconds; unprocessed backlog clears.

---

## 9. Usage settlement backlog elevation (after handoff 09 lands)

### Symptom

Expired unsettled reservations or unexported Polar meter records accumulate.
Owner: AI owner.

### Query

Check settlement backlog in the database (after handoff 09 lands):

```sql
SELECT count(*)
FROM usage_reservations
WHERE status = 'pending' AND expires_at < now();
```

### Action

1. Verify if Polar usage synchronization jobs are running.
2. Check Polar API rate limits and availability.
3. Ensure usage export records retain immutable event identities so replaying
   them does not double-charge.

### Evidence

Pending expired reservations return to 0; all completed usage records are
acknowledged.

---

## 10. Backup freshness and WAL archive lag breach

### Symptom

PostgreSQL WAL archive lag exceeds the 5-minute RPO threshold. Owner: Platform
owner.

### Query

Query PostgreSQL WAL archiver status:

```sql
SELECT last_archived_wal, last_archived_time,
       EXTRACT(EPOCH FROM (now() - last_archived_time)) AS lag_seconds,
       last_failed_wal, last_failed_time
FROM pg_stat_archiver;
```

### Action

1. Inspect database host logs for WAL archiving command failures.
2. Check network connectivity and credentials for the backup target bucket.
3. If disk space on the primary database is filling up due to unarchived WAL
   segments, resolve storage capacity immediately to avoid database freeze.

### Evidence

`lag_seconds` returns to < 300 seconds; `last_failed_wal` clears or ceases
incrementing.

---

## 11. Object-store error rate and reconciliation orphan spike

### Symptom

Elevated 5xx error rate against S3 object storage, or reconciliation job flags
anomalous orphan count growth. Owner: Platform owner.

### Query

Check object storage request error rates from telemetry:

```text
sum(rate(object_store_requests_total{status=~"5.."}[5m]))
/ sum(rate(object_store_requests_total[5m]))
```

Inspect latest reconciliation scan output.

### Action

1. Verify S3 service status and API rate limits.
2. Confirm `DOCUMENT_STORAGE_*` credentials and endpoints are valid.
3. If errors are caused by transient provider degradation, verify that uploads
   fail without creating metadata rows and downloads return typed errors.

### Evidence

Object-store error rate drops below 0.1%. Reconciliation orphan counts stabilize
within nominal operational boundaries.
