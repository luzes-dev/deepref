# Handoff 04: NATS JetStream and transactional outbox foundation

## Goal

Introduce a durable asynchronous transport without moving authoritative job state out of PostgreSQL.

Read `00_PROGRAM.md`. Complete handoff 01 first. This handoff may proceed in parallel with handoffs 02 and 03.

## Architectural rule

PostgreSQL says what is true.

JetStream says that work or an event is available to consume.

Do not move `durable_runs`, `durable_steps`, `durable_attempts`, scientific events, billing state or usage ledgers into NATS.

## Scope

Implement:

- `async-nats` integration;
- NATS/JetStream configuration and readiness;
- stream/consumer provisioning strategy;
- transactional outbox table and persistence API;
- outbox relay;
- message envelope/versioning;
- generic consumer execution shell;
- ACK/NAK/redelivery conventions;
- idempotency conventions;
- tracing/metrics;
- an ADR replacing the old "NATS removed" decision;
- one low-risk production-like pilot message path.

Do not migrate the main durable job queue yet. That is handoff 05.

## Repository reconnaissance

Find:

```text
JobQueue port
jobs table
durable_runs
durable_steps
durable_attempts
worker polling loop
FOR UPDATE SKIP LOCKED
leases
retry scheduling
dedupe keys
architecture checker NATS guard
prior NATS migration/removal docs
```

Document how the existing PostgreSQL execution engine behaves before adding transport.

## NATS client

Use the current maintained Tokio-compatible NATS client. At the time of planning this is `async-nats`.

Add it only in the infrastructure/integration layer permitted by repository boundaries.

Domain and application types must not expose NATS SDK types.

## Stream design

Keep the first design small.

Use workload-oriented subjects rather than service names.

Candidate namespace:

```text
deepref.jobs.ai
deepref.jobs.review
deepref.jobs.documents
deepref.jobs.workflow
deepref.jobs.integration
deepref.jobs.projection
```

Do not create dozens of subjects before a concurrency or retention requirement exists.

Provision streams/consumers deterministically on startup or through an explicit infrastructure bootstrap path.

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

## Message envelope

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

Prefer stable IDs over copying entire aggregates.

Consumers reload authoritative state from PostgreSQL.

## Transactional outbox

Create an outbox table with a stable event ID and delivery state.

Conceptual fields:

```text
id/event_id
subject
message_type
message_version
payload
created_at
published_at
attempts
next_attempt_at
last_error
```

The producer inserts the outbox item in the same PostgreSQL transaction as the authoritative state transition.

## Relay

Implement a horizontally safe relay.

Several relay instances may run concurrently without double-processing becoming harmful.

Use PostgreSQL claim/lease or equivalent atomic selection for relay rows.

Publication must be retryable.

If NATS is down:

- authoritative transactions continue where business rules allow;
- outbox backlog grows;
- relay retries later;
- no work is silently lost.

## Ambiguous publish outcome

Document and handle:

```text
publish timed out
but NATS may have accepted the message
```

Use stable message/event identity and consumer idempotency. Do not assume publish timeout means "not published".

If JetStream message deduplication is used, treat it as an optimization, not the only correctness mechanism.

## Consumer shell

Create reusable infrastructure for:

```text
consume
deserialize envelope
validate version/type
create tracing span
invoke typed handler
ACK on terminal success/no-op
NAK/retry on retryable failure
terminal handling after retry policy
```

Application handlers must not receive NATS message objects.

## Idempotency

Define the rule now:

```text
at-least-once transport
+
idempotent handler
+
authoritative PostgreSQL state/fencing
```

Use aggregate state, unique keys, durable attempts or a consumer inbox when necessary.

Do not promise exactly once.

## Pilot path

Choose a low-risk asynchronous effect that is already eventually consistent, such as a non-critical integration notification or a new synthetic test job.

Prove:

```text
DB transaction -> outbox -> relay -> JetStream -> consumer -> ACK
```

Do not migrate consequential durable review execution yet.

## ADR

Add an ADR that states:

- why NATS is being reintroduced;
- PostgreSQL remains authoritative;
- JetStream is transport;
- transactional outbox is mandatory for DB-triggered publication;
- delivery is at least once;
- current durable-run fencing remains;
- modular monolith remains;
- NATS outage must not corrupt authoritative state.

Update architecture-boundary tooling to permit `async-nats` only in approved layers and remove the obsolete blanket NATS ban.

## Tests

Cover:

- outbox row committed with authoritative transaction;
- transaction rollback removes outbox row;
- duplicate publication;
- duplicate delivery;
- NATS unavailable;
- relay restart;
- two relay instances;
- malformed/unknown message version;
- retryable consumer failure;
- terminal consumer failure;
- trace correlation propagation.

Use integration tests with a real ephemeral NATS instance where feasible.

## Completion criterion

DeepRef can durably publish and consume asynchronous work through JetStream without making NATS authoritative, losing messages across process crashes, or bypassing PostgreSQL transaction boundaries.
