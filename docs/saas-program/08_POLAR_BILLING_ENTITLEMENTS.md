# Handoff 08: Polar billing, commercial state and local entitlements

## Goal

Integrate Polar as DeepRef's Merchant of Record and commercial system while keeping runtime authorization and entitlements local.

Read `00_PROGRAM.md`. Complete handoffs 01, 02 and 03. Handoff 04 should be available for durable provider synchronization.

## Blocking production gate

As of the architecture decision, Polar's published payout-country support did not include Brazil.

Before enabling live production billing, verify current Polar payout eligibility for the actual legal seller/entity.

Development against Polar Sandbox may proceed before that business gate is resolved.

Do not create a foreign legal entity as part of this engineering task.

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

Use the shared integration inbox.

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

- customer provisioning idempotency;
- workspace-to-customer isolation;
- checkout authorization;
- portal authorization;
- duplicate webhook;
- bad signature;
- subscription activation/cancellation projection;
- benefit to entitlement mapping;
- Polar outage while existing entitlements are read;
- reconciliation repairing a missed webhook.

## Completion criterion

A workspace can subscribe and manage billing through Polar Sandbox, DeepRef derives local runtime entitlements from synchronized commercial state, and no normal authorization or feature check requires a Polar network call.
