# Dependency health evidence

## Symptom

`/health/dependencies` reports unavailable storage or degraded JetStream while
`/health/ready` remains ready. API readiness deliberately checks only PostgreSQL
and the required schema. A broker or object-store outage does not revoke durable
API acceptance or change business execution authority.

## Query

Inspect `/health/dependencies` and `/health/ready`. The JetStream detail is present
only when the API is configured for JetStream. It contains counts, never worker,
tenant, endpoint, credential or content identifiers. Each live heartbeat must
have a successful adapter observation no older than 90 seconds. A heartbeat is
live for 60 seconds. Future heartbeat/observation times, unknown old writers,
explicit negative observations, stale evidence and a PostgreSQL worker during a
JetStream cutover prevent a green diagnostic. No live evidence is degraded.

The adapter watch retains the time the actual readiness operation completed.
Heartbeat writes and unrelated transport metrics cannot refresh that time.
This works without OTLP. Database/probe failures are unavailable, never a reused
healthy sample. All dependency probes are bounded by two seconds and run
concurrently; their cancellation may leave backend cleanup to the client pool.

The storage detail reads at most one entry from a listing under the reserved
`__deepref_health__/` prefix, then performs a HEAD against its `read-probe` key.
The listing distinguishes an absent bucket from an absent sentinel object.
NotFound for that object is a completed backend request; listing, authentication,
permission and network errors fail the probe. The probe writes/deletes nothing. Grant the
runtime identity prefix-listing and read-probe permissions when configuring restricted bucket
credentials. This check proves neither upload/delete permission, available
capacity nor content integrity. Full byte/lifecycle and restore contracts are
separate checks.

## Action

Restore the failing dependency and verify fresh observations resume. Check
clock synchronization if future evidence is reported. Verify worker transport
configuration and the served workload pools during cutover. PostgreSQL remains
a valid explicitly selected rollback transport; its mode has no required
JetStream detail. Do not change authoritative claims or business retry budgets
from these diagnostics.

Neither absent/stale heartbeats nor healthy counts prove fleet membership,
complete workload coverage or a drained release. Use the explicit release and
storage/parser drain contracts before contract changes.

## Evidence

Retain sanitized before/after probe payloads, configuration mode and observation
ages. Real tests cover old/new heartbeat writers, unchanged source observation
time across ticks, future/stale/unknown/mismatch cases, read-only API diagnostics
and DB-only readiness. Local tests are not deployed alert delivery or fleet-drain
acceptance.
