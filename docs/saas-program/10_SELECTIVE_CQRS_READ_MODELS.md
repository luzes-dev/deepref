# Handoff 10: selective CQRS, read models and projection workers

## Goal

Separate expensive read concerns from command-side transactional models where profiling and product behavior justify it.

Do not implement full CQRS or event sourcing.

Read `00_PROGRAM.md`. Complete handoff 04. Handoff 05 should be available before production projection workloads are moved to JetStream.

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
transaction writes outbox event
JetStream delivers projection event
projection worker updates read model
```

Projection handlers are idempotent.

Where a projection can be calculated cheaply from authoritative tables after an event, the message may carry only IDs.

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

Use a dedicated JetStream subject/consumer pool for projection work.

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
- projection lag;
- rebuild from authoritative state;
- read model corruption followed by rebuild;
- write then strict read path;
- projection worker outage;
- projection catches up after restart;
- tenant isolation.

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

DeepRef has a small number of justified, rebuildable read models for expensive aggregate queries, command correctness remains in normalized PostgreSQL state, and projection lag or failure cannot corrupt scientific truth.
