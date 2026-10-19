# Handoff 11: API keys, security audit and architecture cleanup

## Goal

Finish the SaaS foundation by adding machine identity, account/security audit, and removing obsolete self-hosted/single-user documentation and code assumptions.

Read `00_PROGRAM.md`. Complete handoffs 02 and 03. Billing-specific audit coverage assumes handoff 08.

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

Candidate events:

```text
auth.login
auth.logout
auth.session_revoked

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

Audit events should be queryable for incident response without becoming part of scientific exports unless explicitly required.

## User deletion

Account deletion/deactivation must preserve:

```text
scientific provenance
security audit history
stable UserId references
```

Remove/anonymize unnecessary PII according to product/legal policy, not by deleting research history.

## Part C: documentation and dead-model cleanup

Perform a repository-wide audit for:

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
PostgreSQL is the queue
removed NATS guard
```

Normative documentation should describe the new architecture:

```text
private SaaS
development/production
WorkOS identity
workspace tenancy
central authorization
scientific policy
PostgreSQL authoritative state
NATS JetStream transport
Valkey ephemeral state
object storage
Polar Merchant of Record
local entitlements
AI reservation/settlement
selective CQRS
```

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

Update the authoritative architecture document to state the final responsibility split.

A concise target:

```text
PostgreSQL       authoritative transaction/scientific/durable state
NATS JetStream   durable work/event transport
Valkey           ephemeral distributed cache/rate/concurrency state
Object storage   large durable blobs
WorkOS           authentication/organization provisioning
Polar            commercial billing/MoR
DeepRef          authorization/scientific policy/entitlements/hard limits
```

Document at-least-once messaging and transactional outbox.

Document selective CQRS, not full CQRS.

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
- user deactivation preserves history;
- repository search/contract tests for obsolete deployment language where practical.

## Completion criterion

DeepRef has a first-class machine identity model using the same authorization system as humans, administrative actions are auditable without exposing secrets, and normative code/docs consistently describe the new private SaaS and distributed-infrastructure architecture.
