# DeepRef SaaS architecture program

## Purpose

This file is the source of truth for the implementation handoffs in this directory. Read it before starting any handoff.

The goal is to evolve DeepRef into a private multi-user SaaS without turning the codebase into a collection of microservices. Keep the existing Rust modular monolith and split infrastructure responsibilities only where the split improves correctness, isolation, or horizontal scaling.

## Target architecture

This is the full target. NATS JetStream and Valkey appear here because they are the selected technologies; they enter the system only when their triggers fire. The launch milestone runs on PostgreSQL and object storage alone.

```text
                         Load balancer
                              |
                     +--------+--------+
                     |                 |
                   API 1             API N
                     |                 |
                     +--------+--------+
                              |
                 +------------+-------------+
                 |            |             |
             PostgreSQL     Valkey      Object storage
             authoritative  ephemeral   durable blobs
                 |
         transactional outbox
                 |
            outbox relay
                 |
          NATS JetStream
      +----------+----------+-----------+-------------+
      |          |          |           |             |
      v          v          v           v             v
     AI        review    documents  integrations  projections
   workers     workers     workers      workers      workers
      |          |          |           |             |
      +----------+----------+-----------+-------------+
                              |
                         PostgreSQL
```

External providers:

```text
WorkOS  -> authentication, sessions, organizations, SSO/SCIM
Polar   -> Merchant of Record, checkout, subscriptions, commercial benefits,
           credits, meters, invoices and tax handling
AI/search/content providers -> accessed through integration boundaries
```

## Fixed decisions

Treat these as requirements unless a later explicit product decision replaces them.

- DeepRef is private-source SaaS.
- Runtime environments are `development` and `production`.
- The product has no self-hosted edition.
- Keep one modular Rust application and repository.
- Do not split domain contexts into independently deployed microservices.
- PostgreSQL remains authoritative for scientific, transactional, tenancy, authorization, billing-projection, usage-ledger and durable-run state.
- NATS JetStream is the selected durable asynchronous transport, not authoritative state. Adopt it when `T-NATS` fires (see "Adoption triggers"). Until then PostgreSQL carries the work and the outbox/inbox contracts stay broker-independent.
- Valkey is the selected store for disposable or reconstructable state. Adopt it when `T-VALKEY` fires. It never holds authoritative state.
- S3-compatible object storage holds large blobs and artifacts.
- Use a transactional outbox for database-to-message publication, and a PostgreSQL integration inbox for inbound provider events. Both are broker-independent.
- Assume at-least-once delivery. Make consumers idempotent and retain database fencing for consequential work.
- Keep `durable_runs`, `durable_steps` and `durable_attempts` authoritative in PostgreSQL.
- Use selective CQRS. Commands keep normalized authoritative state. Add read models only for expensive, frequent, cross-context queries that tolerate bounded staleness, once `T-CQRS` fires.
- WorkOS authenticates. DeepRef authorizes.
- Polar owns commercial state. DeepRef keeps local operational entitlement projections.
- Workspace is the tenant and billing boundary.
- DeepRef canonical IDs are internal strongly typed IDs.
- Scientific roles and blinding policy are DeepRef domain rules, not WorkOS roles or Polar benefits.
- Hard AI/financial limits are enforced locally with atomic reservation and settlement.
- Polar usage is downstream of the DeepRef usage ledger.
- Do not pursue exactly-once messaging. Use idempotency, fencing and replay-safe state transitions.
- Do not add Kafka, Redpanda, RabbitMQ, Elasticsearch/OpenSearch, ClickHouse, Cassandra, Temporal or a graph database without a measured requirement.
- Do not adopt NATS, Valkey or asynchronous read models before their written trigger fires.

## Existing architecture to preserve

The repository already has useful foundations:

- layered crates with domain, application, persistence/integrations and HTTP boundaries;
- PostgreSQL-backed durable runs, steps, attempts, leases, retries and reconciliation;
- a worker process;
- object-store abstraction;
- typed domain job kinds;
- architecture-boundary checks;
- SQLx migrations;
- OpenAPI/Orval generation;
- tracing/telemetry.

Refactor these foundations. Do not replace them with a second orchestration system.

## Program dependency graph

Launch path (first paying workspace):

```text
01 runtime/config foundation
 |\
 | +--> 04a PostgreSQL outbox + integration inbox --> 05a execution/transport split
 | |                                                   (pools, batch claim, typed job scope)
 | +--> 07 object storage (parallel with 02, 03)
 v
02 WorkOS identity
 |
 v
03a workspace tables + project backfill
 |
 v
03b central authorization (PDP/PEP, coverage matrix)
 |
 v
03c principal -> actor + scientific identity continuity       (03a+b+c ship as one release gate)
 |
 +---- business gate: Polar payout eligibility + LGPD policy ----+
 |                                                               v
 |                                                         08 Polar billing
 |                                                               |
 +-------------------------------------> 09 AI usage ------------+
 |                                       (needs 05a; Polar export needs 08)
 +--> 11 Part B audit (starts in 02/03) and Part C docs
 |
 +--> 12 operations acceptance (starts with 01, closes last)
```

Trigger-gated after launch:

```text
04a --> 04b NATS adapter + relay      [T-NATS]
05a + 04b --> 05b JetStream WorkTransport cutover   [T-NATS]
03 --> 06 Valkey shared admission controls          [T-VALKEY]
03 (+ 04a for async projections) --> 10 selective CQRS read models   [T-CQRS]
03 --> 11 Part A API keys               (unless a product requirement pulls it into launch)
```

Suggested implementation order:

1. `01_RUNTIME_CONFIG_FOUNDATION.md`
2. `02_WORKOS_IDENTITY.md`, with `04_NATS_OUTBOX_FOUNDATION.md` part 04a and `07_OBJECT_STORAGE_PRODUCTION.md` alongside
3. `03_TENANCY_AUTHORIZATION.md`: 03a, then 03b, then 03c
4. `05_WORKER_TRANSPORT_MIGRATION.md` phase 05a
5. Business gate: Polar payout eligibility for the actual legal entity and the LGPD privacy/retention/export/deletion policy. Status: LGPD policy, retention and region decided; Polar is not eligible for the seller and the billing provider is undecided (`BUSINESS_DECISIONS.md`)
6. `08_POLAR_BILLING_ENTITLEMENTS.md`
7. `09_AI_USAGE_RESERVATION_METERING.md`
8. `11_API_KEYS_AUDIT_DOCS.md` Parts B and C
9. `12_OPERATIONS_ACCEPTANCE.md`
10. Launch milestone
11. After launch, as triggers fire: 04b, 05b, 06, 10, 11 Part A

Step 02 and step 04a proceed in parallel after 01; WorkOS webhook consumers wait for the 04a inbox. Step 07 proceeds independently after 01 and runs alongside 02 and 03. Step 12 starts with 01 (deployment ownership, CI services) and closes after everything else in the launch set. 11 Part B audit begins in 02 and 03 and 11 owns the remaining coverage and query/retention.

The commercial readiness gate is a business decision, not a code task. Resolve it before 08 starts, not just before live billing: it can change the merchant of record, the plan structure and the data-handling design 08 builds on.

## Cross-cutting rules

### Database transactions

Keep authoritative changes inside PostgreSQL transactions. When a transaction must cause asynchronous work, insert an outbox row in the same transaction.

Never rely on:

```text
commit DB
then publish message
```

or:

```text
publish message
then commit DB
```

as a correctness boundary.

### Messaging

Outbox events and JetStream messages (once 04b ships) should usually carry stable identifiers and routing metadata, not a second copy of authoritative aggregate state.

Example:

```json
{
  "event_id": "...",
  "step_id": "...",
  "workspace_id": "...",
  "trace_id": "..."
}
```

The consumer loads authoritative state from PostgreSQL and rechecks whether the action is still valid.

### Idempotency

Every consequential consumer must handle redelivery safely.

Use one or more of:

- aggregate state transition checks;
- durable-run attempt/fencing state;
- unique idempotency keys;
- consumer inbox records;
- provider idempotency keys.

### Failure model

Design for:

- API restart;
- worker crash during execution;
- worker crash after DB commit but before message ACK;
- NATS outage;
- Valkey outage;
- Polar outage;
- WorkOS outage;
- provider timeout where success is ambiguous;
- duplicate webhook;
- duplicate message delivery;
- out-of-order provider events.

A non-authoritative dependency outage must degrade features without corrupting authoritative state.

### Crate boundaries

Provider and infrastructure SDK types stop in adapter layers.

Domain/application code must not depend directly on:

```text
async_nats
Valkey/Redis clients
WorkOS SDK DTOs
Polar JSON DTOs
object-store provider DTOs
Axum
SQLx
```

unless that dependency is already part of the repository's explicit persistence boundary and the relevant handoff is not yet migrating it.

### Testing

Each handoff must add tests for its own failure modes. Do not defer all distributed-systems testing to the end.

### Tenant isolation

Tenant isolation is proven, not asserted. 03b owns a route/resource coverage matrix covering reads, writes, streams, downloads and machine principals. Every row has a foreign-workspace test. A new route or resource without a matrix row fails CI. Every later handoff that adds a tenant-owned resource or route adds its rows.

### Scientific identity continuity

Canonicalizing identities must not make a previously exposed or previously deciding reviewer appear fresh. 03c owns this invariant against the textual actor IDs compared in `crates/app/migrations/0055_ai_first_blind_controls.sql`. It is launch-blocking: no workspace is released to paying users until 03c's continuity tests pass. Historical provenance stays unchanged.

### Usage accounting

AI usage accounting must never warn and continue. A failed ledger write, a failed reservation or a failed settlement either blocks the operation or leaves a recoverable record that reconciliation settles. 09 owns this rule. It replaces the log-and-ignore behavior in `crates/ai/src/usage.rs` and `crates/ai/src/runtime/hooks.rs`.

### Documentation

Normative docs must describe the architecture after each cutover. Historical review documents remain historical and should be marked non-normative rather than rewritten.

## Adoption triggers

NATS, Valkey and asynchronous read models are selected but not scheduled. Each is adopted when its trigger fires. Handoffs reference triggers by name. The thresholds are proposals. The product owner confirms or replaces them before the launch milestone, and a trigger fires only on the confirmed values.

| Trigger    | Metric                                                                                                     | Proposed threshold                                                          | Measurement window          | Owner         |
| ---------- | ---------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------- | --------------------------- | ------------- |
| `T-NATS`   | Enqueue-to-claim p95 latency per workload pool, after 05a batch claiming                                   | > 2 s                                                                       | 7 consecutive days          | Product owner |
| `T-NATS`   | Share of PostgreSQL CPU spent in claim, lease-renewal and recovery queries                                 | > 20 %                                                                      | 7-day p95 of hourly samples | Product owner |
| `T-NATS`   | Worker replicas in any single pool needed to meet its queue-age SLO (12)                                   | > 8                                                                         | Sustained for 14 days       | Product owner |
| `T-NATS`   | Event fan-out/replay: a second independent consumer of one event family that must replay history it missed | One accepted design doc showing the need and why an outbox dispatcher fails | Per design review           | Product owner |
| `T-VALKEY` | Load on the 06 PostgreSQL limiter tables                                                                   | > 10 % of PostgreSQL CPU or p95 limiter check > 20 ms                       | 7-day p95                   | Product owner |
| `T-CQRS`   | Profiled cost and frequency of one cross-context read                                                      | p95 > 500 ms at > 1 request/s, or > 10 % of PostgreSQL CPU                  | 7 days of production traces | Product owner |

Any one `T-NATS` row firing is enough to start 04b and 05b. The same holds for `T-VALKEY` and 06. `T-CQRS` is per read model: each candidate query needs its own profile before 10 builds a projection for it.

## Launch milestone

Launch is the first paying workspace. It does not require the full target architecture.

Required handoffs:

```text
01  runtime/config foundation
02  WorkOS identity
03  tenancy + authorization (03a, 03b, 03c as one release gate)
04a PostgreSQL outbox + integration inbox
05a PostgreSQL-first execution/transport separation
07  object storage productionization
08  Polar billing + entitlements
09  AI usage reservation + metering
11  Part B (audit) and Part C (docs)
12  operations acceptance
```

Launch also requires the commercial readiness gate (Polar payout eligibility for the actual legal entity; LGPD privacy, retention, export and deletion policy) to be resolved before 08 starts. Decisions so far are in `BUSINESS_DECISIONS.md`.

Not required for launch, adopted when a trigger fires or a product requirement pulls them in: 04b, 05b, 06, 10 and 11 Part A (API keys).

## Review history

`REVIEW_AGREEMENT.md` records the review that produced this revision of the handoffs and the repository facts behind it.

## Program completion criterion

This is the full target architecture, reached after launch as triggers fire.

The program is complete when DeepRef can scale API and worker processes horizontally, isolate workload classes, lose NATS or Valkey without losing authoritative scientific or financial state, authenticate and authorize real users and workspaces, bill through Polar, enforce hard AI limits locally, and serve expensive dashboards from bounded-staleness projections where appropriate, all while remaining one coherent modular application.
