# Handoff 08: Polar billing, commercial state and local entitlements

## Goal

Integrate Polar as DeepRef's Merchant of Record and commercial system while keeping runtime authorization and entitlements local.

Read `00_PROGRAM.md`. Complete handoffs 01 and 02, all of handoff 03 (03a, 03b and 03c as one release gate), and handoff 04a in `04_NATS_OUTBOX_FOUNDATION.md`.

## Commercial readiness gate

Resolve Polar payout eligibility for the actual seller/legal entity in Brazil before this handoff starts substantial work. Small Polar Sandbox spikes may proceed while the gate is unresolved.

Treat this as a business decision, not an engineering workaround. Record the decision before proceeding with substantial billing work.

Do not create a foreign legal entity as part of this work.

## Responsibility split

Polar owns:

```text
customers
products/prices
checkout
subscriptions
customer portal
invoices
tax/Merchant-of-Record obligations
commercial benefits
commercial credits
meters
commercial usage billing
```

DeepRef owns:

```text
Workspace
runtime authorization
local entitlement projection
scientific policy
hard usage enforcement
usage ledger
```

## Customer mapping

Workspace is the billing customer.

Use `WorkspaceId` as Polar external customer identity where the current API supports it.

Persist the Polar customer ID as provider metadata when useful.

Never bill a project or individual user as the primary product tenant.

## Rust integration

Do not add a JavaScript billing sidecar.

Use a small Rust integration adapter through Polar's current versioned REST API.

Keep the adapter behind a DeepRef-owned billing provider port. Do not make Polar's API or data types the domain/application contract; no alternative provider is mandated.

Prefer existing `reqwest`, `serde`, `thiserror` and secret-handling infrastructure.

Model only the endpoints DeepRef uses.

Provider JSON types remain inside the integration layer.

Pin/document API version behavior where Polar supports versioning.

## Local billing projection

Create local tables/models for:

```text
billing account/customer mapping
subscription projection
commercial product reference
billing lifecycle timestamps/status
```

Do not make Polar API calls during ordinary feature authorization.

## Entitlements

Application code must never do:

```text
if plan == "pro"
```

Create typed concepts:

```text
Feature::AiFirst
Feature::Automation
Feature::ApiAccess
Feature::AdvancedExports

Limit::Projects
Limit::WorkspaceMembers
Limit::MonthlyAiCredit
```

Map Polar products/benefits to these DeepRef entitlements.

Store effective local entitlement state.

Conceptual table:

```sql
workspace_entitlements (
    workspace_id,
    key,
    value jsonb,
    source,
    effective_from,
    effective_until,
    updated_at
)
```

Unit tests must be able to construct entitlements without Polar.

## Checkout

Expose an authorized DeepRef endpoint that:

1. authenticates the user;
2. checks workspace billing permission;
3. derives the Polar customer from `WorkspaceId`;
4. creates hosted checkout;
5. returns only a safe redirect URL.

Do not accept arbitrary Polar customer IDs from the browser.

Validate success/cancel return targets.

## Customer portal

Same authorization rule as checkout.

The caller chooses a DeepRef workspace. Server-side state determines the Polar customer.

## Polar webhooks

Use the PostgreSQL integration inbox from 04a in `04_NATS_OUTBOX_FOUNDATION.md`. Webhook receipt and processing must not depend on NATS.

Flow:

```text
raw webhook
 -> verify current Polar signature format
 -> insert idempotently
 -> acknowledge
 -> durable processor
 -> update billing/entitlement projections
```

Subscribe only to event families DeepRef consumes.

Do not assume webhook ordering.

Reconcile state from provider objects/customer state when ordering ambiguity requires it.

## Privacy and data responsibilities

Apply Brazil's LGPD to DeepRef's billing data. Polar handles Merchant-of-Record functions such as checkout, invoices and tax; DeepRef remains responsible for Workspace identity, local billing projections, entitlement/usage data, export and deletion. Record and confirm the parties' LGPD legal roles, processing purposes and notices; Merchant-of-Record duties do not remove DeepRef's responsibilities for its own processing.

Minimize personal data sent to Polar:

```text
WorkspaceId as external customer identity where supported
billing email only when a Polar flow requires it
no workspace member roster or scientific data
```

Treat webhook payloads and billing email as personal data. Keep provider data inside the integration boundary, restrict access to billing projections, and do not copy payloads or personal data into logs unnecessarily.

Include the local billing projection in workspace data export. On workspace deletion, remove or anonymize workspace-linked billing data unless legal retention requires a record to remain. Keep any legally required billing record restricted and detached from the deleted workspace's active product data; delete it when its retention period ends. Define how DeepRef requests provider-side closure or deletion where applicable.

Resolve and record these policy decisions before substantial billing work:

```text
LGPD roles, processing purposes and notice
Polar customer fields and when billing email is required
billing projection and integration-inbox retention periods
workspace export fields and deletion/anonymization behavior
legal-retention fields, access, duration and expiry/deletion process
provider-side closure/deletion responsibility
```

## Polar Customer State

Use provider customer-state APIs for:

```text
reconciliation
operator diagnostics
repair jobs
```

Do not use them on every application request.

## Credits

Polar may own customer-facing commercial credit balances.

Do not yet use Polar's live credit lookup as the hard concurrency boundary.

The AI hard-limit implementation belongs to handoff 09.

Use the immutable per-usage identity defined in `09_AI_USAGE_RESERVATION_METERING.md` for each usage fact exported to Polar. Verify Polar ingestion, correction and billing-period semantics there before enabling retries or aggregation.

## Benefits

Map Polar benefit identifiers to closed DeepRef `Feature` values.

Do not leak benefit IDs into feature code.

## Merchant of Record boundary

Do not build duplicate:

```text
VAT/GST/sales-tax calculation
invoice engine
payment-card storage
charge dispute system
```

DeepRef stores only commercial references needed by product behavior and audit.

## Failure model

Define behavior for:

```text
Polar unavailable during checkout
webhook delayed
duplicate webhook
out-of-order webhook
subscription event received after newer state
portal creation failure
```

Already-known local entitlements must remain usable through transient Polar outages according to product policy.

## Tests

Use Polar Sandbox.

Cover:

- provider port behavior with a fake implementation; entitlements constructible without Polar;
- customer provisioning idempotency;
- outbound customer data minimization, including billing email only when required;
- workspace-to-customer isolation;
- checkout authorization;
- portal authorization;
- duplicate webhook through the 04a PostgreSQL integration inbox;
- bad signature;
- subscription activation/cancellation projection;
- benefit to entitlement mapping;
- workspace export includes billing projections;
- workspace deletion applies recorded retention policy and preserves legally retained billing records;
- Polar outage while existing entitlements are read;
- reconciliation repairing a missed webhook.

## Completion criterion

The commercial readiness gate, including payout eligibility and LGPD policy decisions, is resolved before substantial billing work. A workspace can subscribe and manage billing through Polar Sandbox; the Polar adapter stays behind a DeepRef-owned port, webhooks use the 04a PostgreSQL integration inbox, personal data sent to Polar is minimized, and workspace export/deletion follows the recorded retention policy. DeepRef derives local runtime entitlements from synchronized commercial state, and no normal authorization or feature check requires a Polar network call.
