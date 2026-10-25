# Dodo billing implementation and operating limits

Dodo is an optional configured boundary. Ordinary authorization, scientific
workflows and PostgreSQL AI reservation/settlement do not call it. This foundation
does not turn on a paid-feature paywall. New paid-feature enforcement needs its
own reviewed rollout; unknown evidence cannot grant newly requested paid access
or qualify a workspace for automatic free-account inactivity deletion.

## Contract provenance

The wire DTOs follow the official generated TypeScript SDK at immutable revision
[`2cff19eb469c323b583f1628e516f910c4b44c7f`](https://github.com/dodopayments/dodopayments-typescript/tree/2cff19eb469c323b583f1628e516f910c4b44c7f).
Relevant source paths: `src/resources/customers/customers.ts`,
`src/resources/customers/customer-portal.ts`, `src/resources/checkout-sessions.ts`,
`src/resources/subscriptions.ts`, `src/resources/payments.ts`,
`src/resources/refunds.ts`, `src/resources/webhook-events.ts`, and
`src/core/pagination.ts`. Fixtures are synthetic contract examples, never
recordings of a real merchant or protected sandbox evidence.

Rechecked official documentation on 2026-10-24:

- [Webhook authentication](https://docs.dodopayments.com/developer-resources/webhooks):
  exact bytes signed as ID.timestamp.body; base64-decoded secret, HMAC-SHA256,
  Standard Webhooks v1 signatures, five-minute timestamp tolerance. Rotation
  allows overlapping old/new keys; deployment must remove the previous key after
  the provider's documented 24-hour overlap. Independent Python stdlib vectors
  exercise the Rust verifier.
- [Checkout](https://docs.dodopayments.com/developer-resources/checkout-session):
  hosted URL is nullable; no immediate-charge/stored-payment-method options are
  sent. A redirect never grants entitlements.
- [Portal](https://docs.dodopayments.com/features/customer-portal): fixed test/live
  customer hosts. The pinned portal API passes `return_url` and `send_email` as
  query parameters; the adapter explicitly sends `send_email=false`.

Customer creation has supported metadata but no assumed Polar external-customer
identity. No inspected endpoint guarantee establishes automatic POST idempotency.
The SDK pagination starts at page 1 and continues until an empty page; scans are
bounded to ten pages of at most 100 records. An incomplete scan is unresolved,
not a uniqueness or absence proof. These bounds are application policy, not a
claim of provider snapshot consistency. Concurrently changing remote lists can
remain ambiguous; never issue a second customer POST based on absence.

## Configuration and authority

`DODO_API_KEY`, `DODO_BUSINESS_ID`, `DODO_MODE`, `DODO_WEBHOOK_KEY` are all-or-none.
Mode is explicit `test_mode` or `live_mode`; live additionally requires the
separate `DODO_LIVE_ENABLED` switch. This switch is configuration, not evidence
of seller approval. `DODO_WEBHOOK_KEY_PREVIOUS` supports deliberate key rotation.
`DODO_OFFERINGS_JSON` is a reviewed array of `primary`/`secondary` mappings to
actual provider product IDs and typed local entitlements. It defaults empty.
There are no invented prices, public arbitrary product IDs or unbounded grants.

Fixed API origins and mode-specific hosted hosts are enforced. Redirects are
disabled; connect/request deadlines are 3/10 seconds and bodies are bounded to
1 MiB. Errors are closed/sanitized. The only return paths are `/settings/billing`
and `/settings` on the configured app origin. Provider links are capabilities:
responses are no-store; do not log, cache, or persist them.

Workspace billing management is human-only under central `workspace:billing`.
API keys cannot impersonate a human. User/workspace/membership facts are locked
and decided by the central PDP before dispatch and again before local commit or
link publication. Network calls hold no database transaction. All billing
transactions use 250ms lock and 1500ms statement timeouts; the transaction PDP
borrows the same connection, so pool capacity cannot cause nested acquisition.

## Durable intent and recovery

The account mapping is unique per workspace/provider business/mode. Customer
creation commits a stable UUID operation before dispatch. Simultaneous requests
can dispatch only once. Timeout, invalid response, crash or permission loss after
dispatch leaves `provisioning='ambiguous'`, visible through local diagnostics.
No request blindly recreates the customer. Periodic reconciliation scans supported
customer metadata for that exact operation UUID; it never treats email as tenant
identity. Exactly one match can attach to the still-live generation-fenced tenant.
Zero/multiple/incomplete results require operator investigation. Inspect the
operation UUID, inbox quarantine, review flag and `reconcile_after`; do not delete
the intent to retry. Provider-supported evidence of failure or duplicate cleanup
is required before designing any audited replacement operation. This foundation
intentionally exposes no unsafe force-recreate command.

Free classification requires deliberate local `enroll_free` product-policy
operation and reviewed free limits, with no existing billing history. Missing,
stale, unresolved or detached provider state is never known free. Provider
provisioning atomically removes prior free enrollment. No automatic subscription
absence-to-free transition exists.

## Projection, adverse evidence and reconciliation

Webhooks authenticate raw bytes before JSON parsing and commit to the existing
PostgreSQL integration inbox before ACK. The provider namespace includes business
and mode; configure distinct credentials/signing keys for each mode. No fictional
payload `test_mode` field is assumed. Receipt atomically invalidates a known
mapping's generation. It retains only verified JSON under the existing inbox
30-day processed-payload purge, keeping dedup identities. Invalid signatures or
business IDs create no inbox rows. Unsupported/unmapped payloads remain durable
pending/quarantined rather than granting access.

Existing worker inbox and provider-reconciler lifecycle owns all background work;
there is no billing polling task or replacement outbox. Each reconciliation pass
selects at most 16 due mappings. Current provider reads use finite bounded scans.
A local generation allocated before fetch prevents a stale response from replacing
a newer fetch, webhook, mapping or deletion. Inbox effects and completion share
one transaction and the existing claim fence. Downgrades revoke on the next local
read. Outage preserves only last proven grants for 30 minutes from successful
observation; failures never extend that grace or increase allowances. PostgreSQL
hard financial caps remain independent.

Refund, dispute and credit events open a review hold; they do not establish an
invented universal financial-revocation policy. A current active subscription
alone cannot clear the hold or renew earlier grants. Periodic payment scans also
detect refund/dispute summaries after missed deliveries. An opaque stable UUID
identifies the current adverse payment evidence. `resolve_review` is an internal
operator seam: compare the expected review UUID and record a UUID for the review
of relevant current payment/refund/dispute/credit evidence in the audit trail.
A later changed snapshot opens review again. No public human/API-key endpoint can
force-clear a hold. Credit balance and financial adjustment semantics remain
unresolved; no credit balance is used as per-call AI financial authority.

## Privacy and staged acceptance

The local `BillingView` is a typed export seam without provider references,
credentials, hosted links or raw inbox data. Workspace deletion fences in-flight
HTTP and detaches active customer mapping, projection and workspace reference.
A restricted operation tombstone has a five-year expiry; final purge/export
integration belongs to the subsequent privacy slice. Provider closure/deletion
is not claimed by local detach. Existing financial ledgers remain authoritative.

This is the 08a foundation. Subsequent 08b implements the provider-neutral immutable
09 usage sink and unresolved-adjustment seam; metered charging stays guarded until
dedup retention, ambiguous acceptance, correction, numeric range and period
attribution are proven. Historical events must not be retimed or reidentified.
08c completes privacy export/retention integration. Actual test-account
subscription/portal cycle, seller/product/KYC/bank/payout approval, actual catalog,
privacy publication and hosting decision remain external/pending. Unconfigured
protected checks are not passing checks.
