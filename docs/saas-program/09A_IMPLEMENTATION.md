# Handoff 09a: AI usage reservations

Slice A adds PostgreSQL reservations, hierarchical monthly caps, settlement and workspace-scoped recovery. Provider/model rates come from `deepref-config` and are injected into a generic AI `PriceBook`; usage facts keep provider cost in micro-dollars separate from nullable commercial units, with captured prices and UTC month identity attached for the five-year retention period.

## Reservation and settlement

`MeteredGateway` estimates a conservative input-token bound from the serialized system prompt, user prompt, evidence and schema, plus a fixed request envelope. It combines that bound with the route's output cap, separately billed reasoning cap, other billable components and a rounding margin using the price snapshot captured before dispatch. Missing input or output bounds fail with typed errors before the provider call.

PostgreSQL reserves the full amount in one transaction. It updates conditional period-bucket rows in the order platform, workspace, project, run; every update checks `spent + reserved + requested <= limit`. A failed child or parent update rolls the transaction back. Settlement reverses the unused hold, adds actual cost at the captured rates, records the immutable usage fact and outbox event, and commits them together. If actual cost exceeds the hold or a cap was lowered while the call was in flight, settlement keeps the true cost and blocks the workspace.

Streaming callers can increase an open or observed reservation through `UsageLedger::top_up`. The top-up uses the original price snapshot and period, grows its bounded token amounts, and applies the same ordered conditional updates. Current assistant runs reserve each model turn before dispatch; no production streaming entry point exists yet.

## N-1 overlap rule

N-1 continues writing `ai_usage_ledger`. A database trigger supplies the workspace and UTC month identity, then counts each legacy insert in the platform, workspace and project buckets in platform-to-project lock order. N reservations use the same bucket order, so a committed N-1 spend is visible to the next N reservation. If that insert pushes any hierarchy level past its cap, the trigger records a workspace block in the same transaction.

N-1 still performs check-then-spend before the insert. That old request can race another N-1 request or an N reservation before its ledger row commits; N cannot make that old dispatch atomic. The trigger prevents the spend from disappearing from accounting and blocks later AI work after detecting the overage, but cannot prevent an already-dispatched N-1 call from exceeding the cap. The hard cap applies to N calls; the overlap residual ends when N-1 AI workers are drained.

## Failure handling

| Outcome | Usage record and quota action |
| --- | --- |
| Provider returns usage | Persist observation, then settle the fact and outbox event at reservation-time prices and period. |
| Provider returns an explicit rejection (proved HTTP 4xx, except 408) | Record the rejection reason and release the reservation. |
| Request timeout (HTTP 408), transport timeout, provider 5xx, interrupted call, or unknown outcome | Keep the reservation unresolved. Persist known partial tokens as spent and retain the unrecognized portion as a hold; when usage is unknown, keep the whole hold. |
| Observation or settlement write fails | Leave the reservation open or observed. A failed settlement transaction rolls back the fact and outbox event together for recovery. |
| Actual cost exceeds the reservation or active cap | Record actual cost and overage, release only the existing hold, and block the workspace until an operator waives the block. |

An operator may use `deepref-server ai-usage waive-overage` to clear an active block. `set-cap` records both a limit-change row and an administrative audit event. `reconcile` is a read-only report; recovery writes happen in a workspace-scoped `ai_usage_reconcile` durable job.

## Stale recovery

Each job-backed reservation stores the job ID and claim sequence. Recovery considers an owner stale only when its job is gone, the job attempt is fenced by a different live claim, or the job has left `running` with its lease cleared. A lease timestamp alone never releases a reservation. The worker settles only an `observed` reservation with persisted provider usage; an open or ambiguous reservation with no evidence remains unresolved and continues to hold quota.

## AI invocation inventory

- `AiTaskRunner` in `crates/ai/src/runner.rs` routes each structured call and repair attempt through the production `MeteredGateway`. Worker review tasks and workflow nodes supply workspace, principal, run, step, job and claim-fence attribution.
- The direct structured completion in `services/worker/src/workflows/nodes.rs` supplies usage context and uses that same metered gateway.
- Assistant model turns enter through `run_rig_turn` in `crates/ai/src/runtime/agent.rs`; `crates/ai/src/runtime/hooks.rs` reserves before each model turn and settles or retains the hold on every completion outcome. `services/worker/src/assistant.rs` supplies human/service attribution and the durable job fence.
- Production OpenAI-compatible completion adapters are created through `build_metered_provider` in `crates/ai/src/gateway.rs`. The low-level `RigGateway` has no production callers. `RigEmbeddingGateway` also has no workspace caller; embedding dispatch is not enabled in this slice.
- HTTP assistant routes enqueue durable runs. They do not dispatch model calls; assistant tools execute deterministic host operations.

Structured tracing records reservation acceptance and denial, reserved and settled amounts, actual cost, overage blocks, unresolved liabilities, and reconciliation counts without customer identifiers as metric labels. Provider usage export and delivery monitoring remain downstream work.

## Migrations

- `0120_ai_usage_reservations.sql`: expands the usage ledger, retains workspace/month identity, creates limits, period buckets, reservations, blocks, corrections and outbox tables, backfills existing budgets and spend, and counts N-1 ledger inserts.
- `0121_ai_usage_audit_expand.sql` and `0122_ai_usage_audit_validate.sql`: add AI limit-change and overage-waiver audit shapes using the expand/validate sequence.
- `0123_ai_usage_indexes.sql`, `0125_ai_usage_job_fence_index.sql`, and `0126_ai_usage_ledger_reservation_index.sql`: each builds one concurrent index in its own no-transaction migration.
- `0124_ai_usage_reconcile_job.sql`: maps the workspace recovery job into the AI workload pool.

The ledger drops its cascading project foreign key and retains workspace identity directly, so deleting a project cannot erase a historical usage fact.

## Deferred slices

The provider-neutral outbox is committed with settlement, but billing-provider export, delivery retries and provider-specific backlog/failure metrics are deferred. The leased-slot concurrency table is also deferred. Commercial-unit pricing remains unselected and has no pricing logic in the AI abstraction.
