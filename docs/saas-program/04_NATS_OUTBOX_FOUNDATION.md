# Handoff 04: Transactional outbox, integration inbox and NATS foundation

## Goal

Ship a broker-independent PostgreSQL outbox and integration inbox for launch. Add NATS JetStream only after `T-NATS` fires.

Read `00_PROGRAM.md`. Complete handoff 01 first. Part 04a proceeds alongside 02 and 03. Part 04b is trigger-gated.

## Architectural rule

PostgreSQL says what is true.

Before 04b, an in-process dispatcher is the outbox recipient. It invokes typed handlers or enqueues durable jobs. A row is `delivered` only after every expected recipient's durable effect has committed. Claiming or leasing a row never marks it delivered.

After 04b, JetStream carries messages but does not become authoritative state. Keep the outbox schema and rows; switch the publisher from the in-process dispatcher to the JetStream relay. A JetStream publish confirmation is recorded separately from handler delivery. A row is not `delivered` on claim or publish confirmation alone.

Do not move `durable_runs`, `durable_steps`, `durable_attempts`, scientific events, billing state or usage ledgers into NATS.

## Existing state

`crates/app/migrations/0008_infrastructure_collapse.sql` dropped the former `event_outbox`; `tools/xtask/src/boundaries.rs` currently bans `async-nats`. NATS was deliberately removed. Do not treat either as an accidental omission.

`crates/app/migrations/0066_ai_autonomy_outbox.sql` defines a domain-specific outbox drained by `crates/app/src/ai_autonomy_apply.rs`. Decision: keep `ai_autonomy_outbox` separate from the generic publication outbox. Do not consolidate it as part of this handoff. Any later migration requires its own explicit decision and delivery/idempotency plan.

## Part 04a — PostgreSQL outbox and integration inbox (launch-required)

Implement without a broker:

```text
generic PostgreSQL outbox table and persistence API
atomic claim/lease and retry semantics
in-process dispatcher for typed handlers or durable-job enqueue
WorkOS and Polar webhook integration inbox
```

### Transactional outbox

Create a generic outbox table with stable event identity and delivery state. Insert each outbox row in the same PostgreSQL transaction as its authoritative state change.

Conceptual fields:

```text
id/event_id
message_type and version
payload and routing metadata
created_at
delivery_state
lease_owner and lease_expires_at
attempts and next_attempt_at
published_at when using JetStream
delivered_at
last_error
```

Provide a persistence API to insert rows, claim a bounded batch atomically, renew or release leases, record retry state, and mark completion. Expired leases are reclaimable. Concurrent dispatchers must not hold the same active lease. Use PostgreSQL row locks or equivalent atomic selection.

The in-process dispatcher invokes a typed handler or durably enqueues a job. A handler reports success only after its effect commits. If it enqueues a durable job, the committed enqueue is the effect for this outbox row; job execution has its own state and completion. Mark the row delivered only after every expected recipient's receipt commits, never on claim. Make handlers idempotent because a crash after effect commit but before the receipt commits can cause redelivery.

Retry failures with backoff and preserve enough error state to diagnose them. Do not silently discard rows. Keep the same row contract for 04b: its JetStream relay claims these rows and records broker publication separately; do not add a second NATS-only outbox table.

04a defines no scheduler. Publication retry backoff is not job scheduling: a delayed job's due time stays authoritative in 05's `DurableExecutionStore`, 05a's claim loop picks the job up when due, and 05b's due-time publisher writes the wakeup through these outbox rows when due.

### Delivery completion with fan-out

One message type may have more than one consumer. Model delivery per consumer, not per row.

```text
expected recipients   message type + version declares its consumer names
per-consumer receipt  unique (event_id, consumer) row with state, attempts, next_attempt_at, last_error
aggregate completion  the outbox row is `delivered` only when every expected recipient has a committed receipt
```

A consumer writes its receipt in the same transaction as its own effect or durable job enqueue, never on receipt of the message. A row with some but not all receipts stays pending for the missing recipients only: retries and backoff are tracked per recipient, and a consumer that already holds a receipt treats redelivery as a no-op.

The expected recipient set is part of the message contract and is stable for a given message type and version. Adding, removing or renaming a consumer requires a new message version or an explicit backfill/replay decision; it never silently changes what `delivered` means for rows already committed.

### Integration inbox

Receive WorkOS and Polar webhooks through a PostgreSQL-backed inbox. Verify the provider's signature/authentication before inserting any event. Use a provider-scoped deduplication key, such as `(provider, provider_event_id)`, with a unique constraint.

Return durable `2xx` only after the inbox insert or duplicate check commits. A duplicate already committed for that provider is acknowledged without reprocessing. Track processing state, attempts, next retry time, and terminal/quarantine reason. Retry transient failures with backoff. Quarantine poison events after bounded retries and expose them for operational review and replay.

Do not assume provider event ordering. Make handlers safe for duplicates and out-of-order events; reload authoritative state or apply provider versions/timestamps where supported. Periodically reconcile local projections against WorkOS and Polar so missed, delayed, or quarantined events can be repaired.

## Part 04b — NATS JetStream adapter (trigger-gated by T-NATS)

Start only after at least one `T-NATS` trigger row in `00_PROGRAM.md` has a defined threshold, measurement window and owner, and measured evidence meets it. Cite the fired trigger row and measurements in the ADR. Until then, do not add the dependency or change the boundary rule.

Implement:

```text
async-nats integration in approved infrastructure/integration layers
NATS/JetStream configuration and readiness
stream/consumer provisioning strategy
relay from the existing generic outbox rows to JetStream
message envelope/versioning
generic consumer execution shell
ACK/NAK/redelivery conventions
idempotency conventions
tracing/metrics
ADR reversing the deliberate NATS removal
boundary-check change permitting async-nats only in approved layers
one low-risk production-like pilot message path
```

Do not migrate the main durable job queue yet. That is handoff 05b and also requires `T-NATS`.

### NATS client

Use the current maintained Tokio-compatible NATS client, `async-nats` at planning time. Add it only in the infrastructure/integration layer permitted by repository boundaries.

Domain and application types must not expose NATS SDK types.

### Stream design

Keep the first design small. Use workload-oriented subjects rather than service names.

Candidate namespace:

```text
deepref.jobs.ai
deepref.jobs.review
deepref.jobs.documents
deepref.jobs.workflow
deepref.jobs.integration
deepref.jobs.projection
```

Do not create dozens of subjects before a concurrency or retention requirement exists. Provision streams/consumers deterministically on startup or through an explicit infrastructure bootstrap path.

Document:

```text
retention policy
replica count
max age/size
ack policy
ack wait
max deliver
dead-letter or terminal handling
```

Use environment-specific values through config.

### Message envelope

Define a small versioned envelope owned by DeepRef, for example:

```text
event_id
message_version
kind
occurred_at
trace/request correlation
workspace_id when relevant
resource/run/step identifier
```

Prefer stable IDs over copying entire aggregates. Consumers reload authoritative state from PostgreSQL.

### Relay and ambiguous publish outcome

Use the same generic outbox rows and claim/lease API as 04a. The JetStream relay is a publisher adapter, not a second persistence model. Record successful JetStream publication in `published_at`; do not confuse broker confirmation with handler completion or mark a row delivered on publish alone.

The ordinary publication loop selects rows with `published_at IS NULL`. Rows already published are excluded from ordinary publication retries even while per-consumer delivery is incomplete; a consumer that missed the message is repaired through explicit replay with the same stable event identity, not by republishing the row in the normal loop. Aggregate `delivered` completion still requires every expected recipient's receipt (see Delivery completion with fan-out): the relay records publication, not delivery.

Handle:

```text
publish timed out
but NATS may have accepted the message
```

Use stable message/event identity and consumer idempotency. Do not assume publish timeout means "not published". If JetStream message deduplication is used, treat it as an optimization, not the only correctness mechanism.

If NATS is down, authoritative transactions continue where business rules allow, the outbox backlog grows, and the relay retries later. No work is silently lost.

### Consumer shell

Create reusable infrastructure for:

```text
consume
deserialize envelope
validate version/type
create tracing span
invoke typed handler
ACK on terminal success/no-op after the effect commits
NAK/retry on retryable failure
terminal handling after retry policy
```

Application handlers must not receive NATS message objects. A crash after effect commit but before ACK may redeliver; handlers must be idempotent. Before ACK, the consumer commits its receipt with the effect (see Delivery completion with fan-out); redelivery after the receipt commits is the idempotent no-op case.

### Idempotency

Define the rule:

```text
at-least-once transport
+
idempotent handler
+
authoritative PostgreSQL state/fencing
```

Use aggregate state, unique keys, durable attempts or a consumer inbox when necessary. Do not promise exactly once.

### Pilot path

Choose a low-risk asynchronous effect that is already eventually consistent, such as a non-critical integration notification or a new synthetic test job.

Prove:

```text
DB transaction -> generic outbox -> JetStream relay -> consumer -> committed effect -> ACK
```

Do not migrate consequential durable review execution yet.

### ADR

The ADR must state:

- why the 04b change reverses the deliberate removal recorded by `crates/app/migrations/0008_infrastructure_collapse.sql` and the `async-nats` ban in `tools/xtask/src/boundaries.rs`;
- which `T-NATS` trigger row fired, its threshold, measurement window, owner and measured evidence;
- PostgreSQL remains authoritative and JetStream is transport;
- the generic transactional outbox is mandatory for database-triggered publication;
- delivery is at least once and handler effects remain idempotent;
- current durable-run fencing remains;
- the modular monolith remains;
- a NATS outage must not corrupt authoritative state.

Update `tools/xtask/src/boundaries.rs` to permit `async-nats` only in approved layers and remove the obsolete blanket ban.

## Tests

### 04a — broker-independent

Cover:

```text
outbox row commits with authoritative transaction; rollback removes it
concurrent claims, lease expiry/reclaim, and retry backoff
claim alone never marks a row delivered
delivery is recorded only after the recipient's effect or durable enqueue commits
fan-out: one failing consumer does not re-run a consumer whose receipt committed
the row is delivered only after every expected recipient receipt commits
recipient set is fixed per message type and version; an unexpected consumer is not counted
crash after effect commit safely replays through an idempotent handler
invalid WorkOS/Polar signature creates no inbox row
provider-scoped duplicate webhook and concurrent duplicate receipt
2xx follows inbox commit; failed commit does not acknowledge receipt
processing retry, poison-event quarantine, and replay
out-of-order events and periodic provider reconciliation
```

### 04b — JetStream adapter

Cover:

```text
same outbox rows dispatch through the in-process publisher and JetStream adapter
outbox row commits with authoritative transaction; rollback removes it
duplicate publication and ambiguous publish timeout
relay selection skips rows with `published_at` set; ordinary retries do not republish
a late consumer is repaired by explicit replay with the same event identity
duplicate delivery and idempotent handler effect
NATS unavailable and relay restart
two relay instances and lease recovery
malformed/unknown message version
retryable consumer failure and terminal consumer failure
ACK only after effect commit; publish confirmation alone is not delivery
trace correlation propagation
```

Use integration tests with a real ephemeral NATS instance where feasible.

## Completion criterion

04a is complete for launch when PostgreSQL durably accepts WorkOS and Polar events, safely dispatches committed outbox work without a broker, retries and quarantines failures, and reconciles provider state. Claims alone never count as delivery, and a row completes only after every expected recipient's receipt commits.

04b is complete only after `T-NATS` fires and the ADR cites its evidence. The same outbox rows can then publish through JetStream without making NATS authoritative, losing messages across process crashes, bypassing PostgreSQL transaction boundaries, or republishing rows the relay already published.
