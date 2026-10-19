# Handoff 09: AI usage reservation, settlement and Polar metering

## Goal

Make AI usage safe under concurrency, attributable to SaaS tenants and exportable to Polar without making Polar part of the execution hot path.

Read `00_PROGRAM.md`. Complete handoff 03 and handoff 04a, the broker-independent PostgreSQL outbox. The Polar export depends on 04a, not on the 04b broker relay. Complete handoff 08 before enabling commercial credit/meter synchronization.

## Scope

Implement:

- workspace/user attribution for AI usage;
- provider-cost accounting;
- commercial usage units;
- atomic quota reservation;
- settlement;
- stale reservation recovery;
- local concurrency admission integration;
- durable usage outbox;
- Polar event delivery;
- meter-compatible factual usage events;
- reconciliation and observability.

## Current state

Today AI budgets are per project: `projects.ai_monthly_budget_micros` (added by `crates/app/migrations/0027_ai_usage_and_assistant_plans.sql`) is read, compared and then spent by `crates/app/src/ai_usage.rs`. This is check-then-spend; simultaneous requests each pass the check before any spend lands, so a hard limit is not enforceable.

Ledger write failures are logged and ignored. `crates/ai/src/usage.rs` warns "failed to record AI usage" and `crates/ai/src/runtime/hooks.rs` warns "failed to record assistant usage", then execution continues. Usage can disappear without a trace.

Replace warn-and-continue with a recoverable record. When the usage ledger write fails, do not release the reservation as if the work were free and do not drop the usage: keep the reservation open as unsettled, potentially billable, and let a reconciler settle it from provider evidence or void it with a recorded reason. Every invocation that incurred provider liability must end in a settled or explicitly voided record.

## Local usage is authoritative

Evolve the existing AI usage ledger rather than replacing it.

Every consequential AI invocation should be attributable to:

```text
workspace
project when applicable
user/service principal when applicable
run/step
provider
model
input/output usage
actual provider cost
commercial usage units if defined
timestamp
```

Separate:

```text
provider cost
```

from:

```text
commercial usage units
```

Do not encode today's Polar pricing in the AI provider abstraction.

## Reservation model

Replace check-then-spend.

Reservations are conservative upper bounds, never mean estimates: `max_tokens` for the request or run multiplied by the price-table rate, plus margin for rounding. Under-reserving is a limit breach.

Track conceptually:

```text
limit
spent
reserved
available = limit - spent - reserved
```

Before execution:

```text
atomic reserve conservative upper bound
```

Streaming and agent runs that may exceed their reservation must atomically top up before incurring more liability, or stop. Top-up is the same conditional update as the initial reserve: if remaining headroom cannot cover the increase, the run stops at the last safe boundary and settles what it has already spent. Never grow spend against an unfunded increase.

On success:

```text
release reservation
add actual spend/units
```

On failure:

```text
release reservation
```

Use PostgreSQL constraints/conditional updates or row locking so simultaneous requests cannot oversubscribe the same hard budget.

## Limit hierarchy

Support where configured:

```text
platform safety cap
workspace cap
project sub-limit
run-level safety limit
```

A child limit never creates allowance beyond its parent.

Keep money/credit correctness in PostgreSQL.

Valkey may separately limit simultaneous requests, but not replace the reservation ledger.

## Migration from project budgets

Existing `projects.ai_monthly_budget_micros` values become project sub-limits beneath workspace and platform caps. Backfill:

```text
project sub-limit = existing ai_monthly_budget_micros, unchanged
workspace cap = sum of member project sub-limits at migration time
platform cap = sum of workspace caps at migration time, or a larger configured constant
```

Backfill must not reduce any project's existing allowance. After migration, a child limit never creates allowance beyond its parent; lowering a parent cap may deny previously funded work and must be recorded as a limit change, not silently absorbed.

## Reservation lifecycle

Persist enough identity to reconcile a reservation with the AI invocation/run.

Add expiry/recovery for workers that crash.

Recovery must prove that a reservation is no longer associated with a potentially billable in-flight operation before releasing it.

Do not blindly release all old reservations by timestamp if the provider call may still complete asynchronously.

## Provider failures

Handle:

```text
request rejected before provider accepts
provider timeout with ambiguous outcome
provider returns usage after execution
stream interrupted
worker crash
```

Record enough state to audit settlement decisions.

## Settlement accounting

Prices come from the price table (`crates/ai/src/pricing.rs`, `PriceBook`/`ModelPrice`, micro-dollars per million tokens). Each settlement records the price-table version used to price it. Price the settlement with the version captured at reservation time; if the table changed mid-run, record both versions and the pricing decision. Re-pricing a settled fact creates a correction record, never an in-place edit.

Period rollover: a reservation is charged to the period in which it was opened, even when it settles after the boundary. The usage fact carries exactly one period identity; never split one fact across periods. Reservations that remain open across a boundary stay reserved against their own period until the reconciler settles or voids them.

When actual exceeds reserved anyway (the provider ignored a token cap, or usage arrived above any bound):

```text
record the overage on the settlement
block further AI work for the tenant until covered or explicitly waived
never let spent exceed limit silently
```

The ledger keeps true numbers; enforcement state records the breach.

## Polar event export

After local settlement:

```text
usage ledger commit
+
transactional outbox record
```

The integration worker sends the usage event to Polar.

One settlement produces one local usage fact with one immutable identity. That identity is the commercial event identity; never reissue or rewrite it.

The event should contain factual metadata useful to meters, such as:

```text
provider
model
input_tokens
output_tokens
provider_cost_micros
commercial_units
```

Use `WorkspaceId` as external customer identity where supported.

Use the current Polar event schema, not hard-coded example JSON from planning docs.

## Idempotency

A local usage fact must have a stable commercial event identity.

Before enabling retries, determine the current Polar event-ingestion idempotency/deduplication semantics.

Aggregation or batching of usage facts into Polar events is allowed only after verifying Polar ingestion, deduplication, correction and billing-period semantics. Until then, one event per immutable usage fact.

Handle:

```text
client timed out
Polar may have accepted event
```

without double-charging.

Do not assume retrying a POST is safe merely because it failed locally.

## Meters

DeepRef emits facts.

Polar meters define commercial interpretation.

Do not rename instrumentation every time pricing changes.

If a new price can be represented using existing factual fields, no AI execution code should change.

## Credits

If the product grants Polar commercial credits:

```text
Polar = customer-facing commercial balance
DeepRef = hard runtime enforcement
```

Synchronize commercial allowance into the local entitlement/quota model.

Do not perform a live Polar balance check before every AI call.

## Concurrency admission

Valkey admission control is optional and enters only when handoff 06 exists and `T-VALKEY` fires. Until then, and whenever Valkey is unavailable, use the PostgreSQL fallback: a real leased-slot table (workspace and provider/model slots with lease expiry, owner identity and fencing), not advisory locks. Advisory locks cannot be counted, expired or capacity-bounded and do not implement leased concurrency.

With Valkey, use it for operational concurrency such as:

```text
max simultaneous AI calls per workspace
max calls per provider/model route
```

This is separate from financial reservation.

If Valkey is unavailable, fall back to the leased-slot table with explicit conservative limits.

## Launch gate

Paid AI is not enabled until reservation, settlement, stale-reservation recovery and recoverable usage accounting are all complete and tested. Until then, AI work runs on the unpaid path or is refused; it never runs against a partially built limit.

## Tests

Build real concurrency tests.

At minimum:

1. N requests race for less remaining quota than total estimates;
2. no more than available amount is reserved;
3. actual cost lower than estimate releases capacity;
4. actual cost higher than estimate follows defined settlement policy;
5. provider failure releases correctly;
6. worker crash leaves recoverable reservation;
7. stale reservation recovery;
8. duplicate Polar outbox delivery;
9. ambiguous Polar timeout;
10. local usage remains correct while Polar is unavailable;
11. project sub-limit cannot exceed workspace cap;
12. service and human usage attribution;
13. top-up race: two streaming runs top up against the same remaining headroom and only one wins;
14. usage ledger write failure leaves a recoverable unsettled record instead of dropping usage;
15. reservation spanning a period boundary settles into exactly one period;
16. settlement records the price-table version used.

## Observability

Track:

```text
reservation accepted/denied
reserved amount
settled amount
provider actual cost
stale reservations
recovery actions
unsettled/potentially billable records
overage records and blocks
Polar usage backlog
Polar delivery failures
quota denials by level
```

Do not expose cost/customer identifiers as uncontrolled metric labels.

## Completion criterion

Concurrent AI work cannot exceed DeepRef's hard configured allowance, reservations hold conservative upper bounds and top up before new liability, every AI charge ends in a settled or explicitly voided record carrying its price-table version and period — including charges whose accounting writes failed and were recovered — and Polar receives replay-safe downstream usage without participating in the request hot path.
