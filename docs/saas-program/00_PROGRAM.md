# DeepRef SaaS architecture program

## Purpose

This file is the source of truth for the implementation handoffs in this directory. Read it before starting any handoff.

The goal is to evolve DeepRef into a private multi-user SaaS without turning the codebase into a collection of microservices. Keep the existing Rust modular monolith and split infrastructure responsibilities only where the split improves correctness, isolation, or horizontal scaling.

## Target architecture

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
- NATS JetStream is durable asynchronous transport, not authoritative state.
- Valkey holds only disposable or reconstructable state.
- S3-compatible object storage holds large blobs and artifacts.
- Use transactional outbox for database-to-message publication.
- Assume at-least-once delivery. Make consumers idempotent and retain database fencing for consequential work.
- Keep `durable_runs`, `durable_steps` and `durable_attempts` authoritative in PostgreSQL.
- Use selective CQRS. Commands keep normalized authoritative state. Add read models only for expensive, frequent, cross-context queries that tolerate bounded staleness.
- WorkOS authenticates. DeepRef authorizes.
- Polar owns commercial state. DeepRef keeps local operational entitlement projections.
- Workspace is the tenant and billing boundary.
- DeepRef canonical IDs are internal strongly typed IDs.
- Scientific roles and blinding policy are DeepRef domain rules, not WorkOS roles or Polar benefits.
- Hard AI/financial limits are enforced locally with atomic reservation and settlement.
- Polar usage is downstream of the DeepRef usage ledger.
- Do not pursue exactly-once messaging. Use idempotency, fencing and replay-safe state transitions.
- Do not add Kafka, Redpanda, RabbitMQ, Elasticsearch/OpenSearch, ClickHouse, Cassandra, Temporal or a graph database without a measured requirement.

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

```text
01 runtime/config foundation
 |\
 | +------> 04 NATS/outbox foundation ----> 05 worker transport migration
 |                                         |\
 v                                         | +--> 06 Valkey
02 WorkOS identity                         | +--> 10 selective CQRS
 |
 v
03 tenancy + authorization ----------------+------> 08 Polar billing
 |                                                |
 |                                                v
 +-------------------------------------------> 09 AI usage/metering
 |
 +-------------------------------------------> 11 API keys/audit/docs

07 object storage productionization may run after 01 and in parallel with 02-06.
```

Suggested implementation order:

1. `01_RUNTIME_CONFIG_FOUNDATION.md`
2. `02_WORKOS_IDENTITY.md`
3. `03_TENANCY_AUTHORIZATION.md`
4. `04_NATS_OUTBOX_FOUNDATION.md`
5. `05_WORKER_TRANSPORT_MIGRATION.md`
6. `06_VALKEY_EPHEMERAL_INFRA.md`
7. `07_OBJECT_STORAGE_PRODUCTION.md`
8. `08_POLAR_BILLING_ENTITLEMENTS.md`
9. `09_AI_USAGE_RESERVATION_METERING.md`
10. `10_SELECTIVE_CQRS_READ_MODELS.md`
11. `11_API_KEYS_AUDIT_DOCS.md`

Steps 02 and 04 can proceed in parallel after 01. Step 07 can also proceed independently after 01.

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

JetStream messages should usually carry stable identifiers and routing metadata, not a second copy of authoritative aggregate state.

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

### Documentation

Normative docs must describe the architecture after each cutover. Historical review documents remain historical and should be marked non-normative rather than rewritten.

## Program completion criterion

The program is complete when DeepRef can scale API and worker processes horizontally, isolate workload classes, lose NATS or Valkey without losing authoritative scientific or financial state, authenticate and authorize real users and workspaces, bill through Polar, enforce hard AI limits locally, and serve expensive dashboards from bounded-staleness projections where appropriate, all while remaining one coherent modular application.
