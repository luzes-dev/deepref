# Handoff 09: AI usage reservation, settlement and Polar metering

## Goal

Make AI usage safe under concurrency, attributable to SaaS tenants and exportable to Polar without making Polar part of the execution hot path.

Read `00_PROGRAM.md`. Complete handoff 03 and handoff 04. Complete handoff 08 before enabling commercial credit/meter synchronization.

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

Track conceptually:

```text
limit
spent
reserved
available = limit - spent - reserved
```

Before execution:

```text
atomic reserve estimate
```

On success:

```text
release estimate reservation
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

## Polar event export

After local settlement:

```text
usage ledger commit
+
transactional outbox record
```

The integration worker sends the usage event to Polar.

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

Once handoff 06 exists, use Valkey for operational concurrency such as:

```text
max simultaneous AI calls per workspace
max calls per provider/model route
```

This is separate from financial reservation.

If Valkey is unavailable, use an explicit conservative fallback.

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
12. service and human usage attribution.

## Observability

Track:

```text
reservation accepted/denied
reserved amount
settled amount
provider actual cost
stale reservations
recovery actions
Polar usage backlog
Polar delivery failures
quota denials by level
```

Do not expose cost/customer identifiers as uncontrolled metric labels.

## Completion criterion

Concurrent AI work cannot exceed DeepRef's hard configured allowance, every AI charge is attributable and auditable locally, and Polar receives replay-safe downstream usage without participating in the request hot path.
