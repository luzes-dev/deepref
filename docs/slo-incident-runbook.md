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
sum(rate(deepref_http_requests_total{status_class!="5xx"}[5m]))
/ sum(rate(deepref_http_requests_total[5m]))
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

Query 95th percentile response-header latency grouped by route class. The
instrument `deepref.http.response_header.duration` already excludes responses
with the SSE content type, so no label filter is needed. Prometheus names
assume the standard OTLP translation; verify the ingested spelling first:

```text
histogram_quantile(0.95, sum(rate(deepref_http_response_header_duration_seconds_bucket[5m])) by (le, route_class))
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
SELECT workload_class,
       count(*) AS due,
       max(EXTRACT(EPOCH FROM (now() - available_at))) AS oldest_due_seconds
FROM jobs
WHERE state IN ('queued', 'capability_wait') AND available_at <= now()
GROUP BY workload_class;
```


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
SELECT id, kind, workload_class, attempts, left(last_error, 300) AS last_error, available_at
FROM jobs
WHERE state = 'dead' AND available_at > now() - INTERVAL '30 minutes'
ORDER BY available_at DESC
LIMIT 50;
```

### Action

1. Inspect the stored `last_error` messages on the dead job records.
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
SELECT count(*) AS live_workers, max(seen_at) AS last_heartbeat
FROM worker_heartbeats
WHERE seen_at > now() - INTERVAL '60 seconds' AND seen_at <= now();
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

## 7. Outbox backlog age elevation

### Symptom

Events in the transactional outbox (`outbox_events`, plus the separate
`ai_autonomy_outbox`) exceed age thresholds without delivery. Owner:
Platform owner.

### Query

Check undelivered outbox rows by state (quarantined rows need operator review):

```sql
SELECT delivery_state, count(*),
       max(EXTRACT(EPOCH FROM (now() - created_at))) AS max_lag_seconds
FROM outbox_events
WHERE delivery_state IN ('pending', 'quarantined')
GROUP BY delivery_state;
```

```sql
SELECT count(*), max(EXTRACT(EPOCH FROM (now() - created_at))) AS max_lag_seconds
FROM ai_autonomy_outbox;
```

List and replay quarantined events with `deepref-server deliveries list outbox`
and `deliveries replay` (see the [metrics runbook](metrics-and-alerts-runbook.md#terminal-delivery-recovery)).

### Action

1. Inspect the outbox relay task logs in the worker pool.
2. Check connectivity from the outbox relay to downstream message destinations
   (or NATS if T-NATS triggered).
3. If the relay process crashed, restart worker background tasks.

### Evidence

Max outbox age drops below 30 seconds; pending row count drains to 0.

---

## 8. Inbox backlog age and webhook processing lag

### Symptom

Webhook events from WorkOS or Dodo Payments sit in the incoming inbox without processing.
Owner: Billing owner.

### Query

Query unprocessed inbox rows by provider and state:

```sql
SELECT provider, processing_state, count(*),
       max(EXTRACT(EPOCH FROM (now() - received_at))) AS max_lag_seconds
FROM integration_inbox_events
WHERE processing_state <> 'processed'
GROUP BY provider, processing_state;
```

Quarantined (dead-lettered) rows are replayed with `deepref-server deliveries
replay inbox <id>` after the cause is fixed.

### Action

1. Inspect the webhook inbox processor worker process.
2. Check for schema parsing errors or unexpected payloads from external webhook
   senders.
3. If processing deadlocked, restart the inbox consumer and trigger batch
   reprocessing.

### Evidence

Webhook inbox processing lag returns below 60 seconds; unprocessed backlog clears.

---

## 9. Usage settlement backlog elevation

### Symptom

Nonterminal AI usage reservations age, or usage facts are not exported to Dodo
Payments.
Owner: AI owner.

### Query

Check the settlement backlog (the same rows as the `usage_settlement` backlog
family). Reservations have no expiry; age is diagnostic only:

```sql
SELECT state, count(*),
       max(EXTRACT(EPOCH FROM (now() - opened_at))) AS oldest_seconds
FROM ai_usage_reservations
WHERE state IN ('open', 'observed', 'unresolved')
  AND opened_at <= now() - INTERVAL '5 minutes'
GROUP BY state;
```

### Action

1. Never void or release a reservation because of its age. Check its job fence,
   provider observation and recognized liability first, then resolve it with the
   audited `deepref-server ai-usage resolve` command.
2. Verify the usage export jobs are running, and check Dodo Payments availability
   and rate limits.
3. Ensure usage export records retain immutable event identities so replaying
   them does not double-charge.

### Evidence

The settlement backlog drains through settled or audited resolutions; export
receipts are acknowledged once with their original identities.

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

The application emits no object-store request metric yet; the catalog rows
`object-errors` and `object-reconciliation` are `external_required` with no
source instrument. Until a source exists, use the bounded read probe and the
reconciliation command:

```bash
curl -fsS http://<api-endpoint>:8080/health/dependencies | jq '.object_storage'
```

Inspect the latest `cargo xtask reconcile-storage` output
([reconciliation runbook](object-store-reconciliation-runbook.md)).

### Action

1. Verify S3 service status and API rate limits.
2. Confirm `DOCUMENT_STORAGE_*` credentials and endpoints are valid.
3. If errors are caused by transient provider degradation, verify that uploads
   fail without creating metadata rows and downloads return typed errors.

### Evidence

The read probe is `available` again. Reconciliation orphan counts stabilize
within nominal operational boundaries.
