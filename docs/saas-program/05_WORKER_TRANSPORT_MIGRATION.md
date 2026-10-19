# Handoff 05: separate work transport from durable execution, then migrate to JetStream

## Goal

Two phases.

**05a** (launch-required) runs on PostgreSQL: split `DurableExecutionStore` / `WorkTransport`, run the first `WorkTransport` implementation as the existing PostgreSQL claim loop, add workload pools, bounded batch claiming and typed job scope.

**05b** (trigger-gated by `T-NATS` in `00_PROGRAM.md`): stop using PostgreSQL polling as the primary transport/wakeup mechanism for scalable durable work by adding the JetStream `WorkTransport` adapter, while preserving the existing PostgreSQL lifecycle, leases, attempts and fencing.

Read `00_PROGRAM.md`. Complete handoff 04 first (05a needs 04a's outbox/scheduler contract; 05b needs 04b's NATS foundation).

## Phases

```text
05a  PostgreSQL-first execution/transport separation   (launch-required)
     DurableExecutionStore / WorkTransport ports
     first WorkTransport implementation: the existing PostgreSQL claim loop
     workload_class and deepref-server worker --pool <class>
     per-pool concurrency
     bounded batch claiming up to free pool capacity
     typed job scope: project | workspace | platform
     state-transition matrix for delivery versus authoritative state

05b  JetStream WorkTransport adapter and cutover        (trigger-gated by T-NATS)
     outbox-driven publication of work identifiers
     delivery-to-database claim/fence flow
     retry/redelivery mapping
     controlled cutover from the claim loop
     reconciliation scanner retained as repair
```

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

Move only delivery/wakeup to the transport (JetStream in 05b).

## Target execution flow (05b)

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

Under 05a the same flow runs without the broker: the claim loop delivers by claiming, and the claim is both wakeup and authority.

## Scope

### 05a

- separation of durable execution state from work transport;
- the PostgreSQL claim loop behind the `WorkTransport` port;
- typed project/workspace/platform job scope;
- workload classification, `workload_class`, `worker --pool <class>`;
- per-pool concurrency and bounded batch claiming;
- state-transition matrix enforced in claim/delivery handling;
- graceful drain/shutdown;
- metrics for queue depth, in-flight work and claim latency.

### 05b

- JetStream-backed transport for the existing durable step system;
- worker subscription/consumer configuration;
- delivery-to-database claim/fence flow;
- retry/redelivery mapping;
- compatibility/cutover from the claim loop;
- metrics for lag, in-flight work and redelivery.

## Application ports

Split the current queue abstraction (`JobQueue`, `EnqueueJob` in `crates/application/src/jobs.rs`; `claim_job`, `PostgresJobQueue` in `crates/app/src/jobs.rs`) into two explicit concepts:

```text
DurableExecutionStore
WorkTransport
```

`DurableExecutionStore` owns persisted lifecycle semantics: enqueue, claim/fence, lease renewal, attempts, terminal transitions, recovery.

`WorkTransport` owns delivery mechanics: wakeup of eligible work, redelivery policy, ACK/NAK/term.

The first `WorkTransport` implementation is the existing PostgreSQL claim loop: `run_claim_loop` in `services/worker/src/lib.rs` claiming through `claim_job` in `crates/app/src/jobs.rs` (`FOR UPDATE SKIP LOCKED`). Under 05a, delivery means claiming; the claim remains the authority.

The JetStream adapter in 05b is the second implementation. Delivery then becomes a wakeup hint only; the PostgreSQL claim/fence still grants authority to execute.

Do not leak `async_nats` types into application/domain interfaces. `tools/xtask/src/boundaries.rs` bans `async-nats` today; 05b admits it only in the transport adapter crate and keeps the ban for `crates/application`, `crates/domain` and `crates/app`.

## Typed job scope

`EnqueueJob` requires a `ProjectId` (`crates/application/src/jobs.rs`) and claiming excludes null projects (`claim_job` in `crates/app/src/jobs.rs` filters `project_id IS NOT NULL`). Workspace- and platform-scoped work cannot use the queue unchanged. Introduce a typed scope instead of dummy projects:

```text
JobScope::Project(project_id)
JobScope::Workspace(workspace_id)
JobScope::Platform
```

Migrate `jobs` (created in `crates/app/migrations/0006_evidence_workspace.sql`) to carry the scope, keeping the project reference for project-scoped work. No row may fabricate a project to carry workspace- or platform-level work. Claiming filters on scope, not on `project_id IS NOT NULL`. Authorization and tenancy checks follow the scope kind.

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

Store the class per job as `workload_class` and filter claims by it.

Do not split merely by module name.

The point is independent concurrency, backpressure and failure isolation.

## Worker runtime

Keep one codebase and the existing binary. `apps/server/src/command.rs` parses `deepref-server [serve|worker|all|migrate|--print-openapi]` today; extend that parser with pool selection using the same hand-rolled conventions.

Support process-level selection such as:

```text
deepref-server worker --pool ai
deepref-server worker --pool documents
deepref-server worker --pool integration
```

`deepref-server worker` without `--pool` runs every class, preserving current behavior.

Several replicas of the same pool must safely share the same durable consumer.

## Per-pool concurrency and bounded batch claiming

Replace the one-claim-per-100 ms tick in `run_claim_loop` (`services/worker/src/lib.rs`, `claim_tick`) with bounded batch claiming:

```text
free capacity = pool concurrency - in-flight handlers
batch size    = min(free capacity, configured max batch)
each tick claims at most one batch
```

Never claim in an unbounded inner loop. A full batch does not spin; the next tick re-checks free capacity. A saturated pool claims nothing. Shutdown is observed between claims within a batch and between batches; a batch must never delay signal handling.

Set concurrency separately per pool (see Backpressure). Do not share one global concurrency setting across classes.

## State-transition matrix

Authoritative state decides; delivery decides nothing. The delivery handler maps every row onto the existing delivery vocabulary (`DeliveryAction` in `services/worker/src/delivery.rs`: ACK / NAK with delay / term):

| Authoritative state on delivery      | Action                                                                                              | Transport response       | Who re-publishes                                                        |
| ------------------------------------ | --------------------------------------------------------------------------------------------------- | ------------------------ | ----------------------------------------------------------------------- |
| queued, eligible                     | claim/fence, execute                                                                                | ACK after result commit  | nobody                                                                  |
| queued, not yet eligible (due later) | no-op                                                                                               | NAK with delay until due | outbox scheduler on due (05b); claim loop picks up on due (05a)         |
| claimed by live lease                | no-op                                                                                               | ACK as idempotent no-op  | nobody; the lease holder finishes                                       |
| claimed with expired lease           | no-op on delivery; store lease recovery (`recover_expired_jobs`, `crates/app/src/jobs.rs`) requeues | NAK with short delay     | reconciliation scanner after reset to queued                            |
| stale generation                     | no-op                                                                                               | term                     | nobody; the generation can never become valid                           |
| completed                            | no-op                                                                                               | ACK as idempotent no-op  | nobody                                                                  |
| cancelled                            | no-op                                                                                               | term                     | nobody                                                                  |
| superseded                           | no-op                                                                                               | term                     | nobody; the superseding generation carries the work                     |
| lost fence                           | no-op                                                                                               | term                     | nobody; the fence holder owns execution, reconciliation covers its loss |

Rules that hold across the matrix:

```text
transport redelivery never increments business attempts
business attempts increment only when a claim/fence is accepted
delayed retries persist the due time in PostgreSQL and are published on due via outbox/scheduler
transport delivery counters bound redelivery only (MAX_DELIVERIES, DELIVERY_BACKOFF_SECONDS in crates/domain/src/events/subjects.rs, applied by action_for in services/worker/src/delivery.rs)
exhausted transport deliveries must not strand authoritative queued work
the reconciliation scanner republishes queued work lacking timely transport
```

A persisted due time is authoritative. In 05a `available_at` filtering in `claim_job` is sufficient; in 05b the outbox scheduler publishes the wakeup when the due time passes, and the store's eligibility check still refuses early execution.

## Database claim/fence remains authoritative

Receipt of a transport message (a NATS message in 05b) grants no authority to execute.

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

the handler follows authoritative database semantics and does not execute merely because a message arrived. The matrix above fixes the transport response for each case.

## Retries

Separate:

```text
transport redelivery
business/execution retry
```

Do not accidentally multiply retries. Transport redelivery never increments business attempts; only accepted claim/fence executions do.

The existing durable attempt/retry model should remain the source of truth for consequential retry state.

Use transport redelivery (JetStream in 05b) to recover lost consumers and transient transport failures.

When application logic schedules a later retry generation, persist that state and publish the corresponding new work notification through the outbox.

## DB polling fallback (05b cutover)

Do not retain two permanent primary queue engines.

Use a controlled cutover from the claim loop to the JetStream transport:

1. dual instrumentation;
2. JetStream transport behind a feature/config gate if needed;
3. validate equivalence;
4. switch production transport;
5. retain a reconciliation scanner that detects queued authoritative work lacking timely transport;
6. remove the old hot polling loop once confidence is established.

The reconciliation scanner is repair, not the normal queue. It also covers transport deliveries that were exhausted or lost while authoritative work stayed queued.

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

Shutdown stays responsive during batch claiming: a partially filled batch is abandoned on signal, unclaimed rows stay queued, and no batch outlives the drain deadline.

Do not leave claims permanently running because the process received SIGTERM.

## Observability

Track:

```text
JetStream consumer lag (05b)
pending messages
in-flight handlers
redeliveries
DB claim failures
already-terminal no-ops
execution latency per workload
outbox-to-consume latency
queue depth and oldest due time per workload class
batch claim size and pool saturation
reconciliation republishes
```

## Tests

### 05a (PostgreSQL)

1. pool isolation: a worker with `--pool documents` never claims `ai`-class work; one saturated pool neither drains nor starves another;
2. batch claim bounded by capacity: with free capacity 3 a tick claims at most 3 jobs, a saturated pool claims nothing, and no tick exceeds the configured max batch (never an unbounded inner loop);
3. shutdown during batch: SIGTERM mid-batch stops further claims immediately, drains in-flight handlers within the drain deadline, and leaves unclaimed rows queued and claimable;
4. scoped jobs: workspace- and platform-scoped jobs enqueue, claim, retry and complete with no project row; project-scoped jobs keep requiring a project; no dummy project is ever created;
5. no retry multiplication: repeated redeliveries of one delivery do not increment business attempts; only accepted claim/fence executions do;
6. matrix conformance: one test per matrix row asserting the action and the ACK/NAK/term response, including persisted due time for delayed retries and reconciliation republish after exhausted deliveries.

### 05b (JetStream)

1. worker dies before DB claim;
2. worker dies after claim before side effect;
3. worker dies after side effect/result commit before ACK;
4. duplicate message;
5. stale generation message;
6. cancelled run receives delayed message;
7. two workers race the same logical step;
8. NATS unavailable while new work is queued (PostgreSQL retains the work);
9. NATS returns and reconciliation/outbox drains;
10. graceful shutdown with in-flight work;
11. exhausted transport deliveries do not strand authoritative queued work; the reconciliation scanner republishes it.

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

05a: durable execution state is separated from transport behind `DurableExecutionStore` / `WorkTransport`, the PostgreSQL claim loop serves as the first transport with bounded batch claiming and per-pool concurrency, workers run per workload class via `worker --pool <class>`, and workspace/platform-scoped work runs without dummy projects. This is the launch requirement.

05b: JetStream is the normal transport for durable work, workers scale independently by workload class, and every consequential execution remains guarded by the existing PostgreSQL lifecycle and fencing semantics.
