# Handoff 08: Dodo Payments billing and local entitlements

## Goal and owner decision

Integrate Dodo Payments as the selected Merchant of Record while keeping runtime authorization, scientific policy and hard AI limits local. The legacy filename preserves existing links; Polar is no longer the implementation target.

On 2026-10-23 the owner selected Dodo Payments. Read `00_PROGRAM.md` and `BUSINESS_DECISIONS.md`. Complete 01, 02, 03a–c and the broker-independent PostgreSQL integration inbox from 04a before integrating billing.

The seller remains an individual resident in Brazil, without a CNPJ or foreign entity. Dodo lists Brazil for merchant accounts, supports individual onboarding, and determines individual country eligibility from the verified government ID. Actual product, identity, bank and compliance approval remains external. Test-mode implementation may proceed; live billing must not be enabled until the actual seller and product are approved. See [merchant eligibility](https://docs.dodopayments.com/miscellaneous/accepted-countries-and-territories), [account verification](https://docs.dodopayments.com/miscellaneous/verification-process) and [merchant acceptance](https://docs.dodopayments.com/miscellaneous/merchant-acceptance). Do not create a foreign entity or submit personal verification on the owner's behalf.

## Responsibility split

Dodo owns the commercial catalog, hosted checkout, subscriptions, customer portal, invoices, commercial credit/meter interpretation and Merchant-of-Record functions. DeepRef owns Workspace identity, runtime authorization, typed local entitlements, scientific policy, hard financial reservation/settlement and its immutable usage ledger.

Do not duplicate card storage, invoicing, sales-tax calculation or dispute infrastructure. A browser redirect is never evidence that a payment succeeded.

## Provider adapter and contract evidence

Use a small Rust adapter behind a DeepRef-owned billing port. Existing `reqwest`, `serde`, error and secret-handling infrastructure is sufficient; no JavaScript billing sidecar. Provider JSON and identifiers stay in the adapter. Record the exact API/OpenAPI or SDK source revision used to define fixtures, including mode, endpoints, statuses and error semantics. Reverify current official contracts before implementation rather than copying examples from this handoff.

Explicit test/live mode chooses the corresponding fixed provider endpoint and credentials. Never silently default to live mode. Production validates HTTPS and the allowed Dodo hosts; arbitrary provider base URLs are only available in controlled test fixtures. Requests have bounded connect/overall deadlines, bounded bodies and sanitized errors. Credentials, hosted session links and customer PII must not enter ordinary logs.

Official contract starting points:

- [Checkout sessions](https://docs.dodopayments.com/developer-resources/checkout-session).
- [Subscription integration](https://docs.dodopayments.com/developer-resources/subscription-integration-guide).
- [Webhooks and signature verification](https://docs.dodopayments.com/developer-resources/webhooks).
- [Usage ingestion](https://docs.dodopayments.com/developer-resources/usage-based-billing-guide).
- [Official customer API source](https://github.com/dodopayments/dodopayments-typescript/blob/main/src/resources/customers/customers.ts) and [portal source](https://github.com/dodopayments/dodopayments-typescript/blob/main/src/resources/customers/customer-portal.ts).

These references establish provider behavior, not permission to copy example in-memory webhook processing: DeepRef requires a committed PostgreSQL inbox row before acknowledgement.

## Customer identity and provisioning

Workspace is the billing tenant. Persist a unique workspace-to-Dodo-customer mapping, scoped to provider business and mode. Internal `WorkspaceId` remains canonical; never substitute email or a browser-supplied provider ID for tenant identity.

Use an external identity field only when the current API documents its semantics. Otherwise keep the mapping locally and use a bounded supported metadata correlation field. Do not assume Dodo implements Polar's external-customer API. Customer creation must survive simultaneous requests and ambiguous outcomes: serialize local provisioning intent, retain operation identity, and reconcile supported provider evidence before creating another customer. An undocumented idempotency header is not a correctness guarantee. Unknown outcomes remain visible rather than being retried as unrelated operations.

Minimize outbound PII. Collect a billing email only where the chosen provider flow requires one, with authorized user input. Send no member roster, scientific content, prompts or document names. Do not reuse an email across workspaces to infer they share a billing account.

## Local projection and entitlements

Store billing account mapping, subscription/commercial product references, lifecycle status/timestamps, reconciliation freshness and effective typed entitlements. Provider account/mode identifiers are part of mapping and verification boundaries. Keep any necessary immutable financial history separate from active product state.

Application code uses closed concepts such as `Feature::AiFirst`, `Automation`, `ApiAccess`, `AdvancedExports` and limits for projects, members and monthly AI allowance. It must never branch on a commercial plan name or provider benefit ID. Map configured provider product/entitlement identifiers at the boundary. Unit tests construct effective entitlements without Dodo.

Read effective entitlements locally during ordinary authorization. Define locally known free, paid and unknown billing classification explicitly. An absent or failed projection is not proof that an account is free, especially for automatic inactivity deletion. Revocation and downgrade behavior is explicit and tested. Preserve already-known access through transient provider outages according to a documented bounded grace policy, while refusing unproven grants and increases. Retain PostgreSQL financial checks regardless of commercial credits.

## Checkout and portal

Authorized workspace billing routes derive customer mapping and configured product IDs server-side. The browser selects a closed DeepRef offering, never an arbitrary provider customer, price, quantity or amount.

Validate return URLs against the configured application origin and allowed paths; reject userinfo, unsafe schemes and open redirects. Validate the returned hosted URL against the expected mode/provider host before returning it. Treat it as a short-lived bearer capability: no-store responses, no logs, no shared cached links. Use hosted checkout without stored-card/immediate-charge options unless a separate accountable flow is designed.

Dodo's hosted session contract may return no checkout URL for non-hosted payment flows; do not unwrap a nullable URL or treat it as a completed subscription. Customer portal creation follows the same permission and tenant-mapping rules. Return a protected link without requesting unsolicited provider email delivery.

## Durable webhooks and reconciliation

Receive raw bytes, verify the provider signature and signed timestamp, validate expected business/mode, then insert idempotently into the existing PostgreSQL integration inbox. Acknowledge only after that transaction commits. Deduplicate by provider/business/mode and verified webhook identity. Bound request bodies and signature parsing; malformed or invalid requests create no inbox rows and no unbounded audit storm.

Dodo documents Standard Webhooks: verify the exact raw body with the documented header/signature scheme and replay tolerance. Support deliberate overlapping-key rotation. Use a current vetted library or test the adapter against independently generated signed vectors. Do not parse and reserialize before verification.

The durable inbox handler projects only supported event families. Handle activation, renewal, cancellation, expiry, failed/past-due/on-hold states, plan changes and applicable refund/dispute/credit changes without granting access twice or reviving a newer cancellation. Do not assume arrival order, and do not infer a monotonic provider version from a delivery timestamp. Resolve ambiguous ordering through current provider reads, serialized/fenced local reconciliation and deterministic projection rules. A stale fetch must not overwrite newer committed state.

Periodic bounded reconciliation repairs missed/quarantined webhooks, unknown provisioning outcomes and subscription/entitlement drift. A provider outage grows durable backlog; webhook receipt and local entitlement reads do not require NATS or Valkey. Record safe administrative audit events for checkout, subscription, entitlement and credit changes in the same authoritative transaction.

## Usage export and credits (handoff 09)

Attach the Dodo sink to the existing provider-neutral immutable usage-fact/outbox contract. Do not change reservation/ledger semantics, export rollout fences or identities. Use one event per immutable fact until batching/correction semantics are verified. Numeric meter metadata remains numeric. Stable `event_id` and original event timestamp must survive retry, explicit replay, customer mapping changes and billing-period boundaries.

Dodo's current ingestion guide documents deduplication by `event_id` and a narrow timestamp window: older than one hour or more than five minutes ahead is rejected; historical backfill is unsupported. This limits automatic recovery after a long outage. Never retime old usage, generate a new identity to evade deduplication, or silently mark rejected facts delivered. Persist a distinguishable reconciliation-required outcome, expose operator diagnostics and alert before buffered facts approach the deadline. For ambiguous sends, use supported event lookup to establish acceptance and validate the immutable payload before resending or resolving.

Enable metered charging only after documented tests establish deduplication scope/retention, result granularity, ambiguous-send lookup, billing-period attribution, accepted numeric ranges and supported correction behavior. Missing guarantees stay explicit. Late or corrected financial facts need an audited provider-supported adjustment process; until one is proven, they remain unresolved and commercial metering stays guarded. Operationally resolving an export must not alter scientific provenance or the local immutable billing ledger.

Commercial credits may be projected for display and entitlement policy. A live Dodo balance lookup never replaces atomic local reservation or becomes a prerequisite for each AI call.

## Privacy and retention

Apply `BUSINESS_DECISIONS.md`: DeepRef controls its account/workspace/entitlement data; Merchant-of-Record responsibilities do not remove those obligations. Restrict billing routes and exports to authorized principals. Include the local projection in the account/owned-workspace data export, excluding raw webhook payloads, credentials and unrelated tenants.

On account/workspace deletion, detach active personal identifiers and preserve only the restricted financial fields required for the five-year retention period. Define expiry/purge and provider-side closure/deletion responsibilities. Keep dedup identities while purging integration payloads after 30 days under the existing inbox policy. Record provider roles, processing purposes, international transfer basis and the actual privacy/support contact before launch. Never claim provider deletion occurred without delivery evidence.

## Verification and acceptance

Recorded provider contracts and a fake port cover every PR without credentials. Protected test-mode contract checks run separately with explicit credentials and distinguish unconfigured/skipped from passed. Cover:

- concurrent customer provisioning, ambiguous timeout and reconciliation;
- workspace/business/mode isolation and minimal outbound customer data;
- checkout/portal permissions, safe redirects, nullable hosted responses and secret-safe errors;
- raw-body signature validation, rotation, wrong business/mode, malformed, duplicate and delayed webhooks;
- transactional inbox acknowledgement, crash/replay, out-of-order events and stale reconciliation races;
- all subscription states, closed entitlement mapping and local reads during provider outage;
- billing export/deletion, financial retention and unknown-not-free inactivity classification;
- usage identity replay, duplicate/ambiguous acceptance, numeric metadata, one-hour expiry, period boundaries and correction refusal;
- audit metadata validity, tenant coverage matrix and no provider/PII leakage.

The code slice is complete when a workspace can subscribe/manage billing through the test-mode contracts, synchronize local entitlements durably, reconcile drift and export usage safely through the chosen adapter without adding provider calls to normal authorization/AI execution. Full handoff acceptance additionally requires an actual configured Dodo test-account subscription/portal cycle and protected contract evidence. Live payment/payout approval is an independent launch prerequisite. Do not mark unrun external checks as green.
