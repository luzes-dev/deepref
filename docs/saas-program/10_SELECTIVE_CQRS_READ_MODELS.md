# Handoff 10: selective CQRS, read models and projection workers

## Goal

Separate expensive read concerns from command-side transactional models where profiling and product behavior justify it.

Do not implement full CQRS or event sourcing.

Read `00_PROGRAM.md`. This handoff is trigger-gated by `T-CQRS`: work starts only after a specific read has a measured profile, and after 03 for every projection. Complete handoff 04 (04a for asynchronous projections) before relying on transport. Handoff 05b should be available before production projection workloads are moved to JetStream.

## Definition

For this project, "selective CQRS" means:

- commands continue to mutate normalized authoritative PostgreSQL state;
- ordinary entity reads may continue to read that state directly;
- expensive, frequent, cross-context aggregate queries may read maintained projections;
- projections are rebuildable from authoritative state/events;
- bounded eventual consistency is explicit in the API/UI.

It does not mean separate databases for every context.

## Reconnaissance

Profile or inspect current expensive read paths before creating projections.

Candidate areas include:

```text
project dashboard
review progress
screening queue summary
workspace usage summary
billing/AI usage summary
automation/run summary
activity feed
large audit/report summaries
```

Choose the first projections based on measured query complexity/frequency or obvious cross-context fan-out.

Do not create a read model for `GET report by id`.

## Prefer same-transaction projections first

Update a read model inside the command's transaction when that is cheap. `docs/architecture.md` ("Data and event planes") records the precedent: the v2 screening command writes its event, state projection and project lifecycle status in one transaction, and PRISMA is a live deterministic projection of canonical relational state; graph metrics are recomputed after imports and later ingestions.

Same-transaction projections need no worker, no lag budget and no freshness plumbing, so they stay the default. They are still derived state and must remain rebuildable from authoritative tables.

An asynchronous projection needs a demonstrated benefit:

```text
measured fan-out
an independent consumer that needs retained delivery
recomputation too expensive to run on the write path
```

Absent one of those, keep the projection synchronous.

## Application separation

Clarify application-level command/query seams.

Use names and modules consistent with current code, for example:

```text
CommandHandler/use cases
QueryService/readers
```

Do not introduce a framework merely to call this CQRS.

## Projection transport

When authoritative changes should update an asynchronous projection:

```text
transaction writes authoritative state
transaction writes outbox event (04a)
04a outbox dispatcher delivers the projection event
projection worker updates read model
```

Until `T-NATS` fires, the 04a outbox dispatcher is the transport. Afterwards the same rows are relayed by 04b and delivered through JetStream (05b `WorkTransport`). The projection contract does not change across that cutover.

Projection handlers are idempotent.

Where a projection can be calculated cheaply from authoritative tables after an event, the message may carry only IDs.

## Asynchronous projection gate

An asynchronous projection ships only when all three hold:

```text
tenant isolation      every write and read scoped to a workspace; foreign-workspace rows unreachable
rebuildability        a rebuild path exists and is exercised by a test
explicit freshness    the read exposes fresh/lagging/rebuilding/failed state to the API/UI
```

Do not ship a projection that lacks any of the three, however clear the measured benefit.

## Read-model storage

Start with PostgreSQL tables/materialized projections.

Do not add Elasticsearch, ClickHouse or a second database solely because read models exist.

Design the query interface so a read replica can later be used without changing command semantics.

## Rebuildability

Every asynchronous projection needs a rebuild path.

Examples:

```text
truncate/rebuild one workspace
rebuild one project
rebuild all rows in maintenance mode
```

The read model is not authoritative.

A projection bug must be repairable without reconstructing scientific history from logs outside PostgreSQL.

## Versioning

Include projection version/schema semantics where needed.

If algorithm semantics change, support deterministic recomputation.

Avoid silently mixing rows calculated by incompatible versions.

## Freshness

Expose enough metadata to distinguish:

```text
fresh
lagging
rebuilding
failed
```

Do not show a stale aggregate as if it were transactionally current when the distinction affects user decisions.

For strict reads after a write, either:

- read authoritative state;
- wait for a bounded projection condition;
- or return explicit pending/freshness state.

Choose per use case.

## Read replicas

Prepare query abstractions for optional read replicas, but do not require one.

Classify reads:

```text
primary-required
replica-safe
projection
```

Security-sensitive authorization and immediate write-after-read decisions stay on authoritative state.

## Projection workload isolation

Run projection work in a dedicated `projection` workload pool (05a). Once the transport is JetStream (04b/05b), give that pool its own subject and consumer.

A dashboard rebuild must not starve:

```text
billing webhooks
AI work
scientific review jobs
```

## Tests

Cover:

- idempotent projection event;
- duplicate/out-of-order event where relevant;
- same-transaction projection committed and rolled back with its command;
- asynchronous projection gate: tenant scope, rebuild path and freshness state present before it ships;
- projection lag;
- rebuild from authoritative state;
- read model corruption followed by rebuild;
- write then strict read path;
- projection worker outage;
- projection catches up after restart;
- tenant isolation for every projection read and write.

## Out of scope

Do not implement:

```text
full event sourcing
separate command database
separate query database
Kafka analytics pipeline
search engine
data warehouse
```

unless profiling demonstrates a separate requirement.

## Completion criterion

DeepRef has a small number of justified, rebuildable read models for expensive aggregate queries, cheap aggregates are maintained inside the command transaction instead of asynchronously, every asynchronous projection is tenant-scoped, rebuildable and freshness-aware, command correctness remains in normalized PostgreSQL state, and projection lag or failure cannot corrupt scientific truth.
