# Handoff 05: migrate durable work transport to JetStream

## Goal

Stop using PostgreSQL polling as the primary transport/wakeup mechanism for scalable durable work while preserving the existing PostgreSQL lifecycle, leases, attempts and fencing.

Read `00_PROGRAM.md`. Complete handoff 04 first.

## Core invariant

Keep:

```text
durable_runs
durable_steps
durable_attempts
authoritative job/run state
leases
fencing
accepted attempt identity
reconciliation
```

in PostgreSQL.

Move only delivery/wakeup to JetStream.

## Target execution flow

```text
application transaction
    writes authoritative queued step
    writes outbox message
COMMIT

outbox relay
    publishes step identifier

JetStream
    delivers to workload consumer

worker
    loads step from PostgreSQL
    claims/fences it
    executes
    commits result/state
    ACKs message
```

If the worker crashes after DB commit and before ACK:

```text
JetStream redelivers
worker reloads DB
sees terminal/already-completed state
ACKs as idempotent no-op
```

## Scope

Implement:

- separation of durable execution state from work transport;
- JetStream-backed transport for the existing durable step system;
- workload-specific worker pools;
- worker subscription/consumer configuration;
- delivery-to-database claim/fence flow;
- retry/redelivery mapping;
- graceful drain/shutdown;
- compatibility/cutover strategy from DB polling;
- metrics for lag, in-flight work and redelivery.

## Application ports

Refactor the current queue abstraction if needed.

Prefer two explicit concepts:

```text
DurableExecutionStore
WorkTransport
```

`DurableExecutionStore` owns persisted lifecycle semantics.

`WorkTransport` owns delivery mechanics.

Do not leak `async_nats` types into application/domain interfaces.

## Workload classification

Map current job kinds/handlers to a small set of workload classes.

Candidate classes:

```text
ai
review
documents
workflow
integration
projection
```

Do not split merely by module name.

The point is independent concurrency, backpressure and failure isolation.

## Worker runtime

Keep one codebase and preferably the existing binary.

Support process-level selection such as:

```text
deepref-server worker --pool ai
deepref-server worker --pool documents
deepref-server worker --pool integration
```

Use current CLI conventions.

Several replicas of the same pool must safely share the same durable consumer.

## Database claim/fence remains authoritative

Receipt of a NATS message grants no authority to execute.

The worker must still acquire the PostgreSQL claim/fence required by the existing durable execution model.

If state says:

```text
completed
cancelled
superseded
not yet eligible
invalid generation
lost fence
```

the handler follows authoritative database semantics and does not execute merely because a message arrived.

## Retries

Separate:

```text
transport redelivery
business/execution retry
```

Do not accidentally multiply retries.

The existing durable attempt/retry model should remain the source of truth for consequential retry state.

Use JetStream redelivery to recover lost consumers and transient transport failures.

When application logic schedules a later retry generation, persist that state and publish the corresponding new work notification through the outbox.

## DB polling fallback

Do not retain two permanent primary queue engines.

Use a controlled cutover:

1. dual instrumentation;
2. JetStream transport behind a feature/config gate if needed;
3. validate equivalence;
4. switch production transport;
5. retain a reconciliation scanner that detects queued authoritative work lacking timely transport;
6. remove the old hot polling loop once confidence is established.

The reconciliation scanner is repair, not the normal queue.

## Backpressure

Set concurrency separately per pool.

Examples:

```text
AI          bounded by provider/workspace limits
documents   bounded by CPU/memory
integration bounded by external provider rate limits
review      bounded by application throughput
```

Avoid one global worker concurrency setting.

## Shutdown

On shutdown:

- stop accepting new deliveries;
- allow bounded in-flight completion;
- renew/handle leases correctly while draining;
- NAK or allow redelivery for unfinished work;
- flush telemetry.

Do not leave claims permanently running because the process received SIGTERM.

## Observability

Track:

```text
JetStream consumer lag
pending messages
in-flight handlers
redeliveries
DB claim failures
already-terminal no-ops
execution latency per workload
outbox-to-consume latency
```

## Tests

Build failure-path integration tests:

1. worker dies before DB claim;
2. worker dies after claim before side effect;
3. worker dies after side effect/result commit before ACK;
4. duplicate message;
5. stale generation message;
6. cancelled run receives delayed message;
7. two workers race the same logical step;
8. NATS unavailable while new work is queued;
9. NATS returns and reconciliation/outbox drains;
10. graceful shutdown with in-flight work.

Preserve existing durable-run regression tests.

## Out of scope

Do not implement:

- Valkey;
- CQRS projections;
- WorkOS/Polar business integration;
- Temporal;
- event sourcing;
- separate worker services/codebases.

## Completion criterion

JetStream is the normal transport for durable work, workers scale independently by workload class, and every consequential execution remains guarded by the existing PostgreSQL lifecycle and fencing semantics.
