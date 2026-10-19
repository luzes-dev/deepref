# Handoff 06: Valkey for ephemeral distributed state

## Goal

Introduce Valkey where shared low-latency ephemeral state improves horizontally scaled API/worker behavior.

Valkey must never become required for scientific or financial correctness.

Read `00_PROGRAM.md`. Complete handoff 01. Handoff 04 should be complete before relying on the distributed infrastructure in production, but the Valkey adapter can be developed independently.

## Golden rule

If all Valkey data disappears, DeepRef may become slower or temporarily more restrictive, but authoritative scientific, billing and quota state remains correct.

## Scope

Implement the Valkey adapter and use it only for a small initial set of justified cases:

- distributed rate limiting;
- provider/workspace concurrency admission;
- bounded hot caches where stale data has safe semantics;
- optional request coalescing.

Do not migrate authoritative state.

## Client choice

Evaluate current Rust clients against:

```text
Tokio support
Valkey support
TLS
pooling
cluster/sentinel support if needed
tracing hooks
MSRV
dependency footprint
maintenance
```

Likely candidates include `fred` and `redis-rs`.

Record the choice in a short ADR or architecture note.

## Initial use case 1: distributed rate limiting

Move security/cost-sensitive limits that cannot remain process-local once API replicas scale.

Candidates:

```text
API key request rate
expensive import initiation
AI request admission
webhook abuse protection after signature/token identity is established
other explicitly measured endpoints
```

Use a deterministic algorithm such as token bucket or sliding-window counter.

Do not make a user-controlled value the sole abuse key.

## Initial use case 2: provider/workspace concurrency

Use Valkey to cap concurrent expensive work where the limit is operational rather than financial.

Example:

```text
workspace may have at most N simultaneous AI calls
provider/model route may have at most M simultaneous calls
```

Use TTLs/leases so crashed workers do not permanently consume slots.

Financial budget remains in PostgreSQL.

## Optional caching

Only add caches with clear invalidation or safe short TTL.

Good candidates:

```text
workspace membership/permission summary
effective entitlement summary
small provider metadata
hot project metadata
```

Before caching authorization data, define revocation behavior.

A recently removed member must not retain access for an unacceptable TTL.

Prefer versioned cache keys or explicit invalidation events for security-sensitive cache entries.

## Cache-aside

Use cache-aside:

```text
read Valkey
miss -> read PostgreSQL -> populate
```

PostgreSQL remains authoritative.

Do not implement write-behind for authoritative records.

## Failure behavior

For every Valkey call, define fail-open or fail-closed behavior.

Examples:

```text
authorization cache unavailable -> read PostgreSQL
entitlement cache unavailable -> read PostgreSQL
ordinary content cache unavailable -> read PostgreSQL

security rate limiter unavailable -> choose an explicit conservative fallback
AI concurrency limiter unavailable -> conservative local/DB fallback
```

Never silently make cost controls unlimited because Valkey is down.

## Key design

Namespace keys by environment and purpose.

Do not put raw emails, secrets or arbitrary user content in keys.

Use stable internal IDs.

Document TTL and maximum cardinality for each key family.

## Observability

Track:

```text
cache hit/miss
Valkey errors
rate-limit decisions
concurrency slots
fallback-to-Postgres count
latency
```

Avoid high-cardinality labels.

## Out of scope

Do not store in Valkey:

```text
review decisions
scientific provenance
billing subscription state
usage ledger
AI financial reservation
durable job state
WorkOS canonical membership
Polar credit authority
```

Do not use distributed locks to paper over a missing PostgreSQL invariant.

## Tests

Cover:

- two API instances sharing one limiter;
- TTL expiration;
- crashed concurrency holder;
- Valkey unavailable;
- authorization cache invalidation/revocation;
- stale cache safety;
- no authoritative behavior depends solely on cached value.

Use a real ephemeral Valkey integration test for Lua/atomic operations if the implementation relies on them.

## Completion criterion

Horizontal API/worker replicas share operational limits and useful caches through Valkey, while deleting the entire Valkey database cannot corrupt or permanently lose DeepRef state.
