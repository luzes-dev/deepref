# Handoff 11: API keys, security audit and architecture cleanup

## Goal

Finish the SaaS foundation by adding machine identity, account/security audit, and removing obsolete self-hosted/single-user documentation and code assumptions.

Read `00_PROGRAM.md`. Complete handoffs 01, 02, and 03. Billing-specific audit coverage assumes handoff 08.

Parts B and C are launch-required. Administrative audit schema creation and baseline audit events begin in 02 (`auth.*` events) and 03 (`workspace.*`, `project.*` membership/role/permission events, and bounded denial events). Part B here extends audit coverage (billing and API key events), defines retention, provides query access for incident response, and bounds denial-event volume. Part C removes obsolete self-hosted and single-user documentation and code assumptions across the repository.

Part A (API keys and service principals) is trigger-gated after launch unless an explicit product requirement pulls it into the launch milestone.

## Part A: API keys and service principals

Human sessions and machine credentials are distinct.

Conceptual table:

```sql
api_keys (
    id uuid primary key,
    workspace_id uuid not null,
    created_by_user_id uuid not null,
    name text not null,
    prefix text not null,
    secret_hash bytea/text not null,
    scopes jsonb/text[] not null,
    expires_at timestamptz,
    last_used_at timestamptz,
    revoked_at timestamptz,
    created_at timestamptz not null
)
```

Use current schema conventions.

Generate high-entropy secrets.

Return the full key once.

Store only a verifier.

Use a recognizable prefix such as:

```text
dr_dev_
dr_live_
```

if useful.

Do not use a slow password hash by default for a high-entropy random token unless repository security policy requires it. Use an appropriate keyed/cryptographic verifier and constant-time comparison.

Authentication produces:

```rust
Principal::Service(...)
```

Service principals have explicit workspace scope and permissions.

They cannot impersonate arbitrary humans.

Scientific operations requiring accountable human review reject service principals unless an explicit machine workflow allows them and records machine provenance honestly.

## API-key authorization

Use the same `AuthorizationService` as human principals.

Scopes constrain what the service principal may request.

Workspace membership/tenant boundaries still apply.

Do not create a second authorization implementation for API keys.

## Part B: security/account audit

Create an append-only administrative/security audit stream separate from scientific provenance.

Administrative audit table creation and foundational event emission begin in 02 (`auth.*` events) and 03 (`workspace.*`, `project.*` membership/role/permission events, and initial `authorization.denied` events). Part B extends coverage across the system, establishes retention, bounds denial volume, and provides incident-response query access.

Candidate events:

```text
auth.login
auth.logout
auth.session_revoked
user.provisioned
user.deactivated

workspace.created
workspace.member_invited
workspace.member_removed
workspace.role_changed

project.member_added
project.member_removed

billing.checkout_started
billing.subscription_changed
billing.entitlement_changed
billing.credit_changed

api_key.created
api_key.revoked

authorization.denied
```

Record safe context:

```text
principal
workspace
target
event type
timestamp
request id
safe metadata
```

Never store:

```text
session cookies
access/refresh tokens
API key secret
Authorization header
webhook secret
payment-card data
raw provider credentials
```

### Denial-event volume bounding

Bound `authorization.denied` and authentication rejection events via rate limiting, sampling, or windowed deduplication. Repeated denial storms or credential attacks must not saturate audit storage or degrade database availability.

### Retention and query access

Define an explicit retention policy for administrative audit rows (e.g. 90-day hot query window in PostgreSQL, with automated archival or purge of expired records).

Provide indexed query capabilities for incident response filtered by:

```text
workspace
principal
target
event type
timestamp range
```

Audit events must remain separate from scientific research exports unless explicitly requested for compliance.

## User deletion

Account deletion/deactivation must preserve:

```text
scientific provenance
security audit history
stable UserId references
```

Remove/anonymize unnecessary PII according to product/legal policy, not by deleting research history.

## Part C: documentation and dead-model cleanup

Perform a repository-wide audit for obsolete single-user and untrusted-environment assumptions:

```text
open-source
open source
self-hosted
self hosted
single-user
trusted machine
no authentication
local-user
APP_ENV=local
staging
x-actor-id
x-actor-kind
```

Do not treat PostgreSQL queueing or the NATS boundary ban as obsolete for the launch milestone. Launch keeps PostgreSQL transport (`05a`). Keep the `async-nats` architecture boundary ban in `tools/xtask/src/boundaries.rs` until 04b (`T-NATS`).

Normative documentation should describe the launch architecture and the trigger-gated target architecture separately.

Launch architecture:

```text
private SaaS
development/production
WorkOS identity
workspace tenancy
central authorization
scientific policy
PostgreSQL authoritative state, transactional outbox and durable work transport (05a)
S3-compatible object storage
Polar Merchant of Record
local entitlements
AI reservation/settlement
in-memory / database concurrency controls
```

Trigger-gated target architecture:

```text
NATS JetStream durable work/event transport (04b, 05b; gated by T-NATS)
Valkey ephemeral distributed cache/rate/concurrency state (06; gated by T-VALKEY)
selective CQRS asynchronous read models (10; gated by T-CQRS)
API keys and machine principals (11 Part A)
```

Update documentation again at each trigger-gated cutover as triggers fire.

Historical architecture/review documents remain historical.

Where necessary add a short banner:

```text
Historical review. This document describes the pre-SaaS architecture and is not normative documentation.
```

Do not rewrite historical findings.

## README

Remove claims that DeepRef is:

```text
open-source
single-user
unauthenticated
intended for self-hosting
safe only on a trusted local machine
```

Keep README developer-oriented rather than adding marketing prose.

## Public-distribution remnants

Inspect:

```text
public container publishing
release workflows
installation/self-host docs
contribution language
open-source badges/text
public deployment instructions
```

Remove or revise behavior that no longer matches the private SaaS product.

Do not change legal licensing or repository visibility through code unless explicitly requested.

## Architecture docs

Update the authoritative architecture document to state the launch responsibility split and the trigger-gated target split separately.

Launch architecture:

```text
PostgreSQL       authoritative transaction/scientific/durable state, outbox, and durable work transport (05a)
Object storage   large durable blobs
WorkOS           authentication/organization provisioning
Polar            commercial billing/MoR
DeepRef          authorization/scientific policy/entitlements/hard limits
```

Trigger-gated target (post-launch):

```text
PostgreSQL       authoritative transaction/scientific/durable state and transactional outbox
NATS JetStream   durable work/event transport (after T-NATS trigger: 04b/05b)
Valkey           ephemeral distributed cache/rate/concurrency state (after T-VALKEY trigger: 06)
Object storage   large durable blobs
WorkOS           authentication/organization provisioning
Polar            commercial billing/MoR
DeepRef          authorization/scientific policy/entitlements/hard limits
```

Document at-least-once messaging, broker-independent integration inbox, and transactional outbox.

Document selective CQRS, not full CQRS.

Keep the `tools/xtask/src/boundaries.rs` ban on `async-nats` intact throughout the launch milestone; lift it only when 04b is adopted under `T-NATS`. Update architecture documentation again at each subsequent cutover.

## Security docs

Document:

```text
authentication boundary
public webhook signature boundary
API-key handling
secret-handling expectations
CSRF/origin protection
tenant isolation
incident audit
```

## Tests

Cover:

- valid API key;
- malformed key;
- revoked key;
- expired key;
- wrong workspace;
- insufficient scope;
- service principal cannot impersonate human;
- API key secret never appears in logs;
- security audit records key lifecycle without secret;
- denial-event volume bounding under flood/attack conditions;
- administrative audit query filtering by workspace, principal, and time range;
- audit retention pruning and archival;
- administrative audit excluded from scientific research exports;
- user deactivation preserves history;
- repository search/contract tests for obsolete deployment language where practical, while verifying the NATS boundary ban in xtask remains enforced for launch;
- documentation audit verifying launch architecture (PostgreSQL transport) is documented separately from trigger-gated targets (NATS, Valkey, CQRS).

## Completion criterion

Administrative actions across identity, workspace membership, billing and credentials are comprehensively audited with bounded denial volume and incident-response query access; normative code/docs consistently describe the launch architecture (with PostgreSQL transport and intact NATS boundary guard) separate from trigger-gated targets, with an update plan for each cutover; and machine identity is supported through central authorization when enabled.
