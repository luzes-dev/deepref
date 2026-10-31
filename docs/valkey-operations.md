# Shared operational rate limits

API keys use 600 requests per anchored sixty-second window; each workflow webhook and email endpoint uses a separate 60-request window. The subject is the verified key ID or a workflow resolved from its private endpoint token. Unknown tokens do not create counter rows. Authentication and the live central policy decision remain authoritative on every request; passing this limiter does not authorize access.

PostgreSQL owns the counter for all admitted requests. An atomic row update anchors the window at its first request, increments only grants, and retains a lower ceiling until expiry. There is one row per stored key and two per stored workflow, with deletion cascades. Expired windows reset in place. This is an anchored fixed-window policy, not a claim of an exact rolling-window bound. PostgreSQL statement time determines the window; its row lock serializes competing updates.

Valkey optionally refuses earlier through an atomic single-key Lua script. Every Valkey allow or error still consumes the shared PostgreSQL budget. A reset or a replica unable to reach Valkey cannot establish a second allowance. Different backend windows may temporarily refuse more requests. An ambiguous script timeout is not blindly retried by the application; its eventual execution can consume accelerator capacity even when the caller falls back. No authoritative state is stored only in Valkey.

Set both `DEEPREF_VALKEY_URL` and `DEEPREF_VALKEY_NAMESPACE` to enable acceleration. The namespace is a deployment-unique string of 1–64 lowercase letters, digits or hyphens; do not reuse it across deployments. Keys are `deepref:<namespace>:rate:v1:<closed-kind>:<internal-uuid>`, with no raw credential or user content. Each receives a sixty-second TTL that refused attempts do not extend. Maximum live key cardinality is the verified subjects accessed during that interval. Use a dedicated bounded-memory instance and an explicit eviction policy; eviction is safe because PostgreSQL still caps admitted requests.

Production and remote endpoints require authenticated `rediss://` with normal certificate verification. Development permits plaintext only on loopback. Database 0 is the only supported URL selection; URL query parameters and fragments are rejected. Connection URLs are redacted and driver errors are converted to a static unavailable outcome. The default connection/command deadline is 100 ms, configurable from 10–500 ms. Multiplexed client concurrency is bounded to 64, with zero automatic connection retries. No blocking Redis commands or client payload tracing are enabled. This initial implementation uses a standalone endpoint; managed topology, credentials, network rules and hosting are deployment decisions still pending.

The client connects lazily within a bounded deadline. If Valkey is unavailable, the request uses the shared PostgreSQL ceiling; a later request can initialize the client when the endpoint recovers without restarting the process. An established manager handles later disconnects within its bounded command budget. PostgreSQL counter operations have a two-second outer deadline, 750 ms statement timeout and 250 ms lock timeout. An unavailable PostgreSQL ceiling returns HTTP 503 with `RATE_LIMIT_UNAVAILABLE` and `Retry-After: 1`. Saturation returns 429 and `Retry-After: 60`. Ambiguous commits can consume capacity without admitting the request; they cannot admit uncounted work. Caller cancellations do not refund a counter already consumed.

Run `bash scripts/ci/test-valkey.sh` only with matching disposable `DATABASE_URL` and `TEST_DATABASE_URL`. It requires real Valkey/PostgreSQL and fails if either fixture is absent. It owns its bounded local Valkey container; the caller owns PostgreSQL. The required `integration.valkey` diagnostic probe invokes it in CI. The contracts cover concurrent Lua decisions, TTL, reset, a server-executed script whose reply exceeded the deadline, mixed accelerator/fallback traffic, locked counters, ownership cascades and HTTP authorization after admission.

The facade exposes closed backend/purpose/outcome and latency observations plus fallback counts, without subject identifiers. The default observer is a no-op until the integrated telemetry bridge is installed; hooks alone do not establish deployed metrics or alerts.

This rate-limit slice does not complete handoff 06. The deployed metrics bridge, failure/load acceptance and production cutover remain pending. Bounded concurrency mirrors and displayed-count caching are described below. Existing PostgreSQL AI slots, financial reservations, billing projections, scientific records and durable jobs retain their existing authority.

A rolling upgrade may still contain binaries with the old process-local hook limiter or API-key code without this shared counter. The global operational-limit guarantee begins only after every serving API replica participates in the PostgreSQL ceiling; drain those older replicas before declaring distributed-limit acceptance or activating reliance on the Valkey path. Ordinary schema expansion and legacy handlers remain compatible during overlap, but an unaware binary cannot be counted as sharing the new operational budget. The existing financial/quota PostgreSQL checks remain separate throughout that rollout.

## AI concurrency mirrors

When Valkey is configured, API and worker AI admission use TTL mirrors in addition
to PostgreSQL's workspace and provider-route leases. Every dispatch still needs
both PostgreSQL slots. Valkey can refuse before a database claim, or refuse a
racing claim after the database grants it; the latter reports saturation only
after exact database release succeeds. A failed release remains an explicit
persistence failure with lease expiry as the fallback.

Keys are `deepref:<deployment>:concurrency:v1:workspace:<uuid>` and
`deepref:<deployment>:concurrency:v1:route:<sha256>`. Sorted-set members contain
only holder UUID, generation and slot index. Expired members are removed using
Valkey server time. Each key expires after the lease TTL (60 seconds in
production); each set is bounded by its configured limit, at most 10,000.
Namespaces, route catalog and live workspace count bound key cardinality. There
is no global scan or wildcard deletion on a request path.

PostgreSQL renews first. Optional mirror refresh uses at most one quarter of the
remaining heartbeat interval and cannot turn a confirmed database renewal into
a lost lease. Refresh cannot insert a missing member, so a delayed refresh
cannot resurrect a release. After reset, live calls may be absent from Valkey
until a subsequent acquire; PostgreSQL still counts them. Timed-out writes are
never blindly retried and may leave restrictive ghosts until TTL. Exact member
removal cannot remove a newer generation. PostgreSQL releases before mirror
cleanup; crash and cancellation retain the database expiry fallback.

Deleting Valkey, partitioning a subset of replicas or running database-only
replicas cannot create independent grant budgets. Neither mirror contents nor
its availability changes financial holds, settlements or scientific data.
Closed observer dimensions expose scope kind, operation, outcome and latency;
the deployed metrics/exporter bridge and live TLS/load acceptance remain
separate release requirements. Cluster/sentinel behavior is not claimed.

## Displayed project article counts

With Valkey configured, project list/detail reads cache only the displayed count
of project reports. Names, membership, permissions, billing, scientific decisions
and whole project responses are never cached. Each request still runs the live
central policy and primary tenant/project lookup. A warm cache cannot restore a
deleted project or authorize a revoked membership or API key.

The response `article_count_as_of` states the PostgreSQL observation time.
Snapshots are accepted only within five seconds, with an exact workspace,
project and revision match. `Cache-Control: no-cache`, `no-store` or `max-age=0`
bypasses both cache reads and writes for a primary count. Create/update responses
remain canonical. The existing graph request revision is an additional
invalidation hint; raw and older writers can leave a displayed count stale until
expiry. This count is never used for quotas or consequential commands.

Keys are `deepref:<deployment>:project-count:v1:<workspace>:<project>:<revision>`.
Maximum live cardinality is the distinct project/revision pairs read within five seconds, bounded operationally by the dedicated instance memory limit. Values have a strict 512-byte JSON limit checked on the server before retrieval;
batches contain at most 101 project identities. Each entry has a five-second
TTL. Wrong scope, unknown JSON fields, impossible counts, future/old timestamps,
wrong revisions, malformed or oversized values become misses. Cache connection,
command or memory failures fall back to the bounded PostgreSQL count query. The count facade caps each optional cache operation at 100 ms even when other Valkey purposes have a longer configured deadline.
There is no write-behind, wildcard deletion or permission cache. A reset loses
only this optional accelerator. Different revision entries expire independently.

The real service contracts cover one-connection PostgreSQL fallback, cache hits,
strict bypass, revision hints, server expiry, corrupt values, deployment/tenant
separation and authorization revocation after a warm read. A separately run
broker-pause test covers ambiguous delayed ZSET HOLD execution: PostgreSQL retains
its shared ceiling, release frees its slots, and an eventual restrictive ghost
expires. A client timeout alone is never proof that a mutation did not execute.
The observer/exporter bridge and live TLS/topology/load acceptance remain pending.
