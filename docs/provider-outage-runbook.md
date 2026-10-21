# Provider outage runbook

Operating procedures for third-party dependency degradation, failover, and
outages across PostgreSQL, S3 object storage, WorkOS, Polar, NATS, and Valkey.

## Failure principles

- Third-party dependency failures must leave authoritative state uncorrupted.
- Services degrade features gracefully rather than failing hard or hanging.
- Unrelated endpoints and unaffected workflows continue operating.

---

## 1. PostgreSQL outage or failover

### Symptom

Primary PostgreSQL database fails over or loses network connectivity. API
requests fail with database pool timeouts. Worker job processing stalls.

### Query

Check API `/health/ready` endpoint:

```bash
curl -is http://<api-endpoint>:8080/health/ready
```

Check database connection pool errors in application logs:

```text
grep -E "pool timed out|connection refused|terminating connection due to administrator command" api.log worker.log
```

### Action

1. `/health/ready` returns HTTP 503 during failover switchover and automatically
   recovers to HTTP 200 once the new primary is reachable.
2. In-flight jobs are fenced by lease expiry (`WORKER_CLAIM_LEASE_SECS`) and
   reach exactly one terminal state.
3. Committed outbox and inbox rows survive across the failover (after handoff
   04a lands).
4. Replaying jobs or events has no duplicate consequential effect due to
   idempotency keys and unique constraints.
5. If automatic failover stalls, notify the infrastructure team to complete
   replica promotion.
6. Do not restart application replicas if connection pools automatically reconnect.

### Evidence

`/health/ready` returns HTTP 200 OK. Worker heartbeats reappear in
`/health/dependencies`. No orphaned half-committed state or duplicate job
outcomes exist.

---

## 2. Object storage outage or degradation

### Symptom

S3-compatible object store experiences service degradation, elevated latency,
or returns 5xx HTTP responses.

### Query

Inspect dependency health (object-store health is reported in
`/health/dependencies`, **not** in `/health/ready`):

```bash
curl -fsS http://<api-endpoint>:8080/health/dependencies | jq '.object_store'
```

Verify that `/health/ready` remains HTTP 200:

```bash
curl -fsS http://<api-endpoint>:8080/health/ready
```

### Action

1. Verify API readiness stays healthy so non-storage routes remain operational.
2. Ensure uploads fail cleanly without committing metadata rows in PostgreSQL
   (atomic failure: no dangling metadata pointing to non-existent blobs).
3. Ensure downloads return a typed error to clients rather than an unbounded
   HTTP hang or socket leak.
4. Asynchronous document deletion jobs retry automatically with exponential
   backoff.
5. Unrelated endpoints (review browsing, screening without blob fetches, user
   management) continue serving requests normally.

### Evidence

`/health/ready` reports ready. No orphan metadata rows are created in `documents`.
Storage deletion jobs resume and drain once object storage recovers.

---

## 3. WorkOS identity and webhook outage

### Symptom

WorkOS API returns HTTP 5xx, connection timeouts, or reports an active platform
outage on their status dashboard.

### Query

Check authentication error rates on the login endpoints:

```text
sum(rate(http_requests_total{route="/auth/login",status=~"5.."}[5m]))
```

Inspect webhook arrival metrics for WorkOS webhook events (after handoff 04a lands).

### Action

1. Existing valid user sessions continue working uninterrupted via local JWT
   validation (as permitted by handoff 02). Active users are not logged out.
2. New login attempts return a typed "provider unavailable" error response,
   which is explicitly distinct from "unauthenticated" or invalid credentials.
3. Webhook delivery retries from WorkOS buffer in the incoming inbox without
   loss (after handoff 04a lands).
4. Once WorkOS service restores, execute the user/organization reconciliation
   sweep to sync any missed out-of-band updates.

### Evidence

Existing users maintain active sessions. New logins show clear provider-down
messaging. Webhook backlog drains cleanly with zero data loss after recovery.

---

## 4. Polar billing and checkout outage

### Symptom

Polar billing API returns 5xx errors or is unreachable. Checkout sessions cannot
be created.

### Query

Check billing checkout failure logs:

```text
grep -E "polar_client_error|checkout_session_failed" api.log
```

Inspect AI screening and assistant execution logs to confirm they are not blocked.

### Action

1. Locally known entitlements remain usable (per handoff 08 failure model);
   existing customers do not lose access to their tier features during the outage.
2. Checkout endpoints fail cleanly, informing users that payment services are
   temporarily unavailable.
3. Usage export backlog accumulates safely in the database with immutable
   identities (after handoff 09 lands).
4. **Critical invariant:** No AI call waits on Polar. AI inference, screening,
   and assistant turns execute without synchronous billing blocking.

### Evidence

AI screening and assistant workflows operate with zero interruption. Checkout
fails with clean user feedback. The usage export backlog drains to Polar after
provider restoration with no double-billed events.

---

## 5. NATS JetStream outage (conditional: trigger T-NATS)

### Symptom

NATS cluster is unreachable. Worker message bus publishers fail to emit events.

### Query

Check outbox backlog depth in PostgreSQL:

```sql
SELECT count(*)
FROM outbox_events
WHERE published_at IS NULL;
```

### Action

1. The transactional outbox in PostgreSQL continues accepting committed events
   within the application transaction.
2. Outbox relay tasks enter exponential backoff retry loops. No committed work
   is lost.
3. Once NATS cluster recovers, the relay resumes streaming messages in order.
4. Consumers use deduplication keys so no consumer applies a message twice.

### Evidence

PostgreSQL outbox backlog drains to 0 upon reconnection. Consumer logs confirm
duplicate message protection.

---

## 6. Valkey coordination and cache outage (conditional: trigger T-VALKEY)

### Symptom

Valkey / Redis coordination cluster is offline or connection refused.

### Query

Check cache error logs and connection metrics in `/health/dependencies`:

```bash
curl -fsS http://<api-endpoint>:8080/health/dependencies | jq '.valkey'
```

### Action

1. Rate limiting and concurrency admission control fall back immediately to
   PostgreSQL leased slots with conservative concurrency limits.
2. Application caching falls back to authoritative database queries.
3. No authoritative state changes or data corruption occurs during degraded mode.
4. When Valkey restores, reconnection is automatic.

### Evidence

Admission limits function under conservative PostgreSQL lease limits. System
operates in degraded mode without downtime or data corruption.
