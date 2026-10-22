# AI usage reservations

Slice A adds PostgreSQL reservations, hierarchical monthly caps, settlement, and workspace recovery. The AI layer keeps provider cost in micro-dollars separate from nullable commercial units. Every settled fact retains its captured price version, provider identifiers, and UTC billing month for the five-year retention period.

## Request bounds and supported routes

The metered gateway prepares and freezes the provider request before it opens a reservation. For OpenAI-compatible structured completions, it renders evidence into the prompt, builds the complete chat-completions body, serializes that body once, hashes those exact bytes, and sends those same bytes. The bound starts with that final JSON body, then adds the route's maximum fixed template text, per-message markers, each rendered tool schema, and each tool or argument wrapper. A 1.25 multiplier covers JSON formatting differences between the wire body and the model's text rendering. UTF-8 bytes are a conservative token upper bound because every nonempty token encoding contains at least one byte. Multimodal message parts are refused because those routes have no reviewed expansion bound.

Rig assistant turns convert the dispatch request into Rig's OpenAI chat-completions wire request before measuring it. The final wire JSON includes conversation history, tools, schemas, parameters, and rendered evidence; the same template expansion and safety multiplier are then applied. The exact wire request is fingerprinted before dispatch. Assistant calls reject model changes, provider options, additional parameters, and option overrides at the hook boundary. Both paths require an explicit output limit no greater than the route's 2,048-token ceiling.

`crates/ai/src/pricing.rs` contains an explicit provider/model capability table beside `PriceBook`. Each entry has its own source-derived byte ceilings; adding a route without a documented template derivation is unsupported.

| Route | Published template or encoder | Fixed template bytes | Per-message bytes | Per-tool wrapper bytes | Per-argument wrapper bytes |
| --- | --- | ---: | ---: | ---: | ---: |
| `opencode-go/glm-5.3-flash` | [GLM-5.3-Flash chat template](https://huggingface.co/zai-org/GLM-5.3-Flash/raw/main/chat_template.jinja) | 1,024 | 128 | 128 | 64 |
| `opencode-go/glm-5.3` | [GLM-5.3 chat template](https://huggingface.co/zai-org/GLM-5.3/raw/main/chat_template.jinja) | 1,024 | 128 | 128 | 64 |
| `zai/glm-5.3` | [GLM-5.3 chat template](https://huggingface.co/zai-org/GLM-5.3/raw/main/chat_template.jinja) | 1,024 | 128 | 128 | 64 |
| `opencode-go/kimi-k2.6` | [Kimi-K2.6 chat template](https://huggingface.co/moonshotai/Kimi-K2.6/raw/main/chat_template.jinja) | 512 | 128 | 128 | 64 |
| `opencode-go/deepseek-v4-flash` | [DeepSeek-V4-Flash encoder](https://huggingface.co/deepseek-ai/DeepSeek-V4-Flash/raw/main/encoding/encoding_dsv4.py) | 2,048 | 128 | 128 | 128 |

The fixed ceiling includes the largest emitted system/reasoning preamble and tool-declaration wrapper. Per-message ceilings cover the largest role, turn, and end markers. Per-tool ceilings cover the declaration and tool-call wrappers; per-argument ceilings cover the GLM argument tags and DeepSeek DSML parameter envelope. These values round up the source-derived UTF-8 sizes to the next power-of-two guard band. The estimator separately serializes every wire tool entry to count its schema bytes, and counts each tool call and argument occurrence. Cached prompt tokens are priced at the uncached input rate, which is conservative. Hosted search, retrieval, image, audio, attachments, and other separately billed tools are not enabled. Unknown routes and routes with undeclared components fail with a typed unsupported-route error; there is no fallback price.

The prepared request fingerprint is stored with the reservation and top-ups. OpenAI-compatible parameter additions are rejected, including overrides of `model`, `messages`, `max_tokens`, `max_output_tokens`, reasoning budgets, `n`, or billable tools. Assistant dispatch rejects nonempty `additional_params` and provider options before reserving.

## Caps, locks, and admission

When a transaction touches both a project row and usage buckets, it locks the project row `FOR NO KEY UPDATE` first. It then uses the hierarchy order platform bucket → workspace bucket → workspace admission mutex → project bucket → lifetime run bucket. This applies to project `set-cap`, legacy ledger writes, and the workspace-assignment trigger. Final settlement, audited resolution, and deletion also take the same per-project lifecycle advisory lock before hierarchy buckets or detach/cleanup checks. Cap updates and reservations initialize missing buckets with `ON CONFLICT DO NOTHING`, lock the rows, and read each active limit from the locked bucket itself. They never restore a cap from a pre-lock read. The workspace admission row is locked within both reserve and top-up transactions and by any operation that sets or clears a block, including settlement.

A project receives its configured sub-limit when it is created. Creating or deleting a project never changes workspace or platform caps. Only the one-time initial migration seeds workspace and platform caps from child totals. Monthly platform, workspace, and project buckets share the same UTC month. A run bucket uses the durable run or review ID and a lifetime sentinel period, so model invocation IDs and month rollover do not grant a fresh run allowance.

When `ai_hard_caps_enforced` is false, the new reservation path fails closed with `HardCapsDisabled` before paid provider dispatch. It does not send an unmetered N request. The migration defaults this flag to false. N-1 continues its existing behavior, while the legacy ledger trigger counts each N-1 insert in the monthly hierarchy and records the last insert time.

## N-1 rollout and rollback

1. Apply the expand schema migrations and deploy the N binary with paid AI disabled on the N path. Keep N-1 AI dispatchers running only while the legacy trigger is accounting their ledger writes.
2. Run `deepref-server ai-usage backfill --batch-size 500` repeatedly until the output reports `complete: true`. Each batch is its own transaction and stores a marker with the bucket increments, so reruns resume without double-counting. The command refuses to enable caps while any legacy row is unmarked.
3. Drain every N-1 API and worker that can dispatch paid AI. Wait for the configured quiet window; keep all legacy dispatchers stopped during the window. The enable command also requires no legacy ledger insert during that window, a complete backfill, and no paid AI dispatch job queued or running (recovery and export maintenance jobs do not block admission activation).
4. Enable the cap path with `deepref-server ai-usage enable-hard-caps --quiet-window-seconds <seconds> <operator-user-uuid>`. The action is audited. N paid AI admissions remain fail-closed until this command succeeds.
5. For rollback, first disable paid AI dispatch at the deployment level and drain N dispatchers. Then run `deepref-server ai-usage disable-hard-caps <operator-user-uuid>`; this audited command sets the gate false, so N admissions fail closed. Only after that may N-1 be rolled back in. Its writes remain counted by the legacy trigger. Keep a capable N recovery worker deployed for outstanding liabilities; cap suspension does not stop recovery.

The period-start `CHECK (period_start IS NOT NULL)` is installed as `NOT VALID` during expand. It remains unvalidated in this release so populated production databases can finish the bounded backfill before validation runs.

### after backfill: add validation migration

After production has completed `deepref-server ai-usage backfill`, a later release should add a contract migration that validates the period-start check. Keep the check `NOT VALID` until that migration is deployed; do not validate it during the 09a expand migrations. The eventual `SET NOT NULL` change remains a separate contract decision.

## Settlement and recovery

A successful response without complete usage counters is unknown liability, not zero cost. The provider request ID, response ID, and any usage returned are written to the reservation in a short transaction before settlement begins. Observation writes retry three times on fresh ledger transactions with bounded backoff. If all attempts fail, an error log includes the reservation and provider IDs plus usage counters, without prompt content, and the reservation is marked unresolved when possible. The original hold remains if the unresolved write also fails; operators can use the logged evidence with `ai-usage resolve`. Settlement stores the immutable usage fact and adjusts all buckets atomically. If the fact insert fails, the earlier observation remains available to recovery.

Complete `observed` rows are selected in their own recovery page, so old `open` or `unresolved` liabilities cannot starve them. The reconciler settles only a job-backed observation whose job is gone, fenced by a newer claim, or no longer running with its lease cleared. An `open` or `unresolved` reservation without sufficient evidence remains held indefinitely; age never releases quota. A provider usage-lookup port exists, but no adapter is enabled yet.

Resolve a reservation only after reviewing provider or other authoritative evidence:

- `deepref-server ai-usage resolve <reservation-uuid> --settle <micros> --operator <operator-user-uuid>` records a known cost.
- `deepref-server ai-usage resolve <reservation-uuid> --void --reason <text> --operator <operator-user-uuid>` releases a liability only when no usage evidence or recognized cost exists.

Both actions write an administrative audit event. Settlements clamp the remaining hold at zero before release, including when recognized cost already exceeds the original reservation. Overage settlement records the full cost and activates the workspace admission block. `set-cap` records the old and new limit; `waive-overage` clears a block with an audited operator decision.

Deleting a project with open, observed, or unresolved reservations detaches its usage identity and keeps project/run buckets until every reservation is terminal. Settlement or audited resolution normally garbage-collects those detached buckets. Each workspace reconciliation also sweeps a bounded batch of detached projects whose liabilities are all terminal. Detach and final settlement serialize on a project lifecycle lock so one always sees the other's committed state and performs cleanup. Deleting a project never changes a parent cap.

## AI invocation inventory

- `AiTaskRunner` routes structured calls and repair attempts through `MeteredGateway`. Worker review tasks and workflow nodes supply workspace, principal, durable run, step, job, and claim-fence attribution.
- Workflow node completions use the same metered gateway.
- Assistant model turns enter through `run_rig_turn`; dispatch reserves each model turn and completion persists usage before settlement. Worker assistant runs supply human/service attribution and the durable job fence.
- Production OpenAI-compatible adapters are created through `build_metered_provider`. The low-level `RigGateway` has no production callers. `RigEmbeddingGateway` has no workspace caller, so embedding dispatch is not enabled in this slice.
- HTTP assistant routes enqueue durable runs. They do not dispatch model calls; assistant tools execute deterministic host operations.

Tracing records reservation acceptance and denial, reserved and settled amounts, actual cost, overage blocks, unresolved liabilities, and reconciliation counts without customer identifiers as metric labels. Provider-neutral export ships in Slice 09b: see [Export](#export) below. Slice 09a writes no export outbox rows, and settlement still writes none — export emits by replaying retained usage facts.

## Export

Slice 09b exports settled usage to the billing provider through a provider-neutral port. The provider is undecided (`BUSINESS_DECISIONS.md`), so no code outside a future adapter names a vendor.

### Identity

Each immutable usage fact id is the commercial event identity: the ledger row id for a settled fact, the correction record id for a correction. That id is also the outbox event id and the provider idempotency key. It is never reissued and never rewritten. A correction record exports as its own fact — its payload carries the corrected cost and price version and references the original in `correction_of` — and the original event is never edited.

### Emission

Export emits by **replaying retained usage facts**. Settlement commits the ledger row and its bucket adjustments only; it writes no export row and never waits for export, so the AI hot path is untouched. The replay keysets retained facts by `(settled_at, id)` and inserts one `ai_usage.fact` v1 outbox event per fact through the 04a `insert_outbox_event` contract, with `event_id` equal to the fact id. That insert is `ON CONFLICT (event_id) DO NOTHING`, so re-emission is a committed no-op, exactly one event exists per fact, and two replay tasks running concurrently converge on the same rows. A fact is *pending emission* when its outbox event does not exist yet: emission state is derived from the outbox row itself rather than from a cursor table, so no scan cursor can fall behind a late-committing settlement transaction and skip a fact forever. Facts settled while export is disabled are emitted by a later replay, never lost.

Usage facts and correction records share one id namespace, enforced in the database (`ai_usage_fact_ids`, with triggers on both fact tables) so every writer is covered, including N-1 ledger inserts. An outbox event that holds a fact id but is not that fact's export event is an identity collision, never a successful earlier emission: the replay raises `UsageExportError::EventIdentityConflict`, the backlog counts it under `conflicting_facts` instead of `emitted`, and the fact stays unreleased until an operator resolves the collision.

The replay is bounded and resumable: one run inspects at most its limit (pages of 200 rows per source), and any run may stop at a page boundary. It runs twice:

- as a durable `ai_usage_export_replay` job, platform-scoped on the 05a execution store and pooled in the `ai` workload class beside `ai_usage_reconcile`. The worker reconciler schedules one job per minute while export is enabled, and each run drains up to 20 batches;
- as an operator command, `deepref-server ai-usage export-replay [--since <rfc3339>] [--limit <rows>]` (default limit 10,000 rows per invocation).

A separate `ai_usage_export_cursor` or per-fact emitted-marker table was considered and rejected (handoff 09, export state options a and b). Such a table would still need the outbox event id's uniqueness to stop double emission, could drift from the outbox, and a cursor could fall behind a late-committing settlement transaction and skip a fact forever. The chosen option (b) — emission inside the replay with `event_id = fact id` — makes the outbox row itself the emission record, so replay is idempotent by construction.

The event payload carries facts only: fact id, workspace id (the external customer identity), project id when applicable, principal *kind* (`user` or `service` — never which principal), provider, model, input/output/reasoning tokens, `provider_cost_micros`, nullable `commercial_units`, price version (null only for facts settled before price versions existed), the one UTC period, `settled_at`, and `correction_of` for corrections. No email, no names, no prompt text, no secrets, no principal identity.

### Sink contract

`UsageExportSink` (`crates/application/src/usage_export.rs`) delivers one fact event and returns exactly one of `Accepted`, `Duplicate`, `RetryableFailure`, `PermanentFailure`, or `Ambiguous`. The `billing_usage_export` outbox consumer calls the sink and always sends the fact id as the provider idempotency key.

- `Accepted` or `Duplicate` commit the consumer receipt in the same transaction; only then is the outbox row marked delivered.
- `RetryableFailure` retries with the same event id on the outbox's capped backoff (30 s doubling to 15 min).
- `PermanentFailure` quarantines the recipient immediately for review; the row keeps its payload and is never silently discarded.
- `Ambiguous` (for example a client timeout, where the provider may have accepted) is **not** success. The event is retried with the same event id and the row stays pending until the sink returns `Accepted` or `Duplicate`. Retries after an ambiguous outcome can double-charge a provider that does not deduplicate by event id, so **a real adapter must verify the provider's event-ingestion idempotency and deduplication semantics before retries are enabled** (spec "Idempotency"). Aggregation or batching of facts into fewer provider events is likewise allowed only after the provider's ingestion, deduplication, correction and billing-period semantics are verified; until then it is one event per immutable fact.

`DisabledUsageExportSink` is the default while no billing provider is configured. It never claims success: every event it sees stays pending and retries through the outbox backoff, so nothing is lost and nothing is dropped as delivered. Usage fact events use a 1,000-attempt budget (`USAGE_EXPORT_MAX_ATTEMPTS`), so an unavailable provider retries for roughly ten days before an operator review, instead of quarantining an ordinary outage.

### Disabled mode

`DEEPREF_USAGE_EXPORT_ENABLED` (default `false`) is the switch, and while it is off the emitter emits nothing at all and the worker registers no `billing_usage_export` consumer. No rows exist, no delivery attempt is spent, and nothing can be quarantined by waiting: retained facts simply sit in the ledger and a later replay emits every one of them. The alternative — registering a consumer that always answers retryable — was rejected because it burns the per-consumer attempt budget into quarantine for a state that is not an incident. `DEEPREF_USAGE_EXPORT_REPLAY_BATCH_SIZE` (default 500, range 1–1000) bounds one replay batch.

### Rollout gate (N-1 compatibility)

An N-1 binary claims by workload class and dead-letters any returned job kind it cannot decode. Migration 0138 fences `ai_usage_export_replay` and `ai_usage_reconcile` at the database claim boundary, including workers with stale or absent heartbeats:

- N claims in a transaction, sets `deepref.job_kinds` with `set_config(..., true)`, and advertises the exact decoder set before updating jobs. The setting is transaction-local and cannot leak to another borrower of a pooled connection. N also filters kinds before `LIMIT`.
- Protected pending jobs use `capability_wait`, outside N-1’s `state = queued` candidate query. This prevents a high-priority protected job from consuming every old worker’s `LIMIT` and starving ordinary jobs. N selects both pending states and declares its decoders. A trigger also suppresses unadvertised transitions to running. `RETURN NULL` means N-1 receives no row to decode and spends no attempt. The trigger suppresses expired protected-lease updates from unadvertised N-1 recovery too; N recovery declares capabilities and filters known kinds before updating. Heartbeat capabilities remain diagnostic; recent heartbeat presence is not evidence of fleet membership or absence of old claimers.
- Export scheduling still needs both `DEEPREF_USAGE_EXPORT_ENABLED` and the durable `ai_usage_export_activated` flag. `enable-export` checks the legacy-write quiet window, while the database independently protects job claims.
- `disable-export` takes the exclusive job-write advisory lock `(913, 9)` before the rollout-state row lock, closes the gate and parks pending export jobs (including `capability_wait`) as `failed`. A statement trigger takes the shared side before job row locks for every INSERT/UPDATE. Ordinary workers hold compatible shared locks, so they remain concurrent with each other. The exclusive side is never taken while a writer holds the shared side and never waits for the rollout row: every rollout transition acquires both locks nonblocking (`pg_try_advisory_xact_lock`, `FOR UPDATE NOWAIT`) and returns `RolloutBusy` after releasing its locks on contention, so it can never deadlock against an ordinary transaction that owns a project row before its job INSERT or against an N-1 ledger writer that owns the rollout row before a project row. A successful `disable-export` is therefore final: earlier writers are committed and parked by its pass, later enqueues, retries, graceful releases and expired-lease requeues park themselves at the row guard whenever the committed export gate is closed. Running handlers may finish; the export processor rechecks both gates.
- Liability recovery is independent of admission enforcement. `disable-hard-caps` refuses new N admissions but preserves recovery jobs. The scheduler keeps creating workspace recovery jobs and the processor settles saved observations; unresolved liabilities retain their holds and remain visible. N-1 is fenced from this kind even during rollback.

**Rollout prerequisites:** deploy all expand migrations through 0140 before running this binary, and retain the database fences through application rollback. A capable N worker must serve the AI pool to make progress on recovery and export. To roll the rest of the fleet back, disable export, suspend paid AI admission and drain paid dispatchers; retain a capable recovery worker until outstanding observations settle. If no capable worker can remain, run `deepref-server ai-usage reconcile` repeatedly with N tooling and monitor unresolved holds until N workers return. A full N-1 fleet cannot execute the new recovery handler; the DB retains its jobs and liabilities safely during that interruption.

The fence adds one shared advisory-lock acquisition per job write statement and a small row-trigger branch; N claim also adds one transaction-local setting statement. No throughput benchmark is claimed. Protected rows are excluded from N-1 candidates before its `LIMIT`, so ordinary work continues even when all capable workers have drained. Use capable workers for sustained processing of the AI pool. Rollout commands never wait for open job-write transactions: they fail fast with `RolloutBusy`, release their locks, and are retried (a few fresh attempts internally, then by the operator) until one succeeds. A transition that holds the exclusive lock must stay short and must not hold job row locks before writing jobs, so its parking pass can never wait behind a writer that is waiting for the guard. Every rollout command takes the advisory lock before the rollout row, and job statements acquire the shared lock before job row locks, avoiding the disable/job-row lock inversion. Database migrations and operator SQL must follow this ordering when changing the export gate.

Migration 0133 holds `SHARE ROW EXCLUSIVE` locks on both fact sources before the first audit/backfill snapshot, through trigger installation at commit. N-1 inserts wait and then enter the shared registry through the new trigger. This requires a bounded migration window for the retained-fact scan: it blocks writes to these tables, while reads continue. Existing namespace collisions abort migration for operator repair.

### Observability

Export counts facts emitted, delivered, duplicates, ambiguous outcomes, retryable failures and permanent failures (`UsageExportCounters`, fixed fields with no label dimension), and the backlog is reported as SQL: emitted and pending events, unemitted facts and corrections, unattributed facts, and the oldest pending and unemitted ages. Counter labels are fixed and carry no workspace, project, user or cost identifiers. Facts whose workspace cannot be resolved are counted and warned about, never exported to no customer and never hidden.

### Runbook: enabling a billing provider later

1. Confirm the billing provider and its event-ingestion idempotency, deduplication, correction and billing-period semantics in writing (handoff 08, `BUSINESS_DECISIONS.md`).
2. Implement `UsageExportSink` as the provider adapter and verify its dedup semantics against a sandbox before enabling retries; wire it where `services/worker` currently builds `DisabledUsageExportSink`.
3. Apply migrations through 0140 and deploy a capable N worker serving the AI pool. Drain all N-1 paid AI dispatchers before enabling paid admissions; database fencing protects new maintenance kinds during mixed-worker deployment.
4. Run `deepref-server ai-usage enable-export [--quiet-window-seconds <seconds>] <operator-user-uuid>`. It refuses while N-1 AI activity is inside the quiet window. Claim safety comes from the database fence, independently of heartbeat freshness. This opens the durable `ai_usage_export_activated` gate the scheduler requires.
5. Set `DEEPREF_USAGE_EXPORT_ENABLED=true` on the worker and restart it. The reconciler starts scheduling `ai_usage_export_replay` jobs and the consumer drains what the replay emits.
6. Run `deepref-server ai-usage export-replay` to drain any backlog immediately; rerun until the summary reports `"complete": true`.
7. Watch the export counters and the backlog ages. A growing `oldest_pending_age_seconds` is the provider outage signal; `permanent_failure` counters, quarantined `ai_usage.fact` rows and `conflicting_facts` need operator review (`outbox_events.last_error` names the cause).
8. **Rollback** (including rolling the binary back to N-1): run `deepref-server ai-usage disable-export <operator-user-uuid>` **first**. It clears the gate and parks every queued replay job in a state no claim accepts, so N-1 can never claim and dead-letter the kind. If it reports that a rollout transition is in progress, it changed nothing and released its locks; run it again until it reports its parked-job count. Then drain paid dispatchers and roll back, retaining the recovery path described above. `deepref-server ai-usage disable-hard-caps <operator-user-uuid>` suspends admissions while recovery continues.

## Migrations

- `0120_ai_usage_reservations.sql` expands the ledger, creates limits, monthly and lifetime buckets, reservations, block/admission and rollout state, backfill markers, and legacy-write accounting. It seeds parent caps once from existing child limits, creates project buckets only for workspace-assigned projects, and does not scan the full ledger.
- `0121_ai_usage_audit_expand.sql` and `0122_ai_usage_audit_validate.sql` add and validate audit shapes for limit changes, hard-cap rollout, and reservation resolution.
- `0123_ai_usage_indexes.sql`, `0125_ai_usage_job_fence_index.sql`, and `0126_ai_usage_ledger_reservation_index.sql` each build one concurrent index in a no-transaction migration.
- `0124_ai_usage_reconcile_job.sql` maps the workspace recovery job into the AI workload pool.
- There is no 0127 validation migration in this slice. The `NOT VALID` period-start check remains until the follow-up validation migration after production backfill.

Slice 09b (export):

- `0130_ai_usage_export.sql` maps the `ai_usage_export_replay` job into the AI workload pool beside `ai_usage_reconcile`, and indexes correction records for the replay keyset.
- `0131_ai_usage_export_keyset_index.sql` builds the `(created_at, id)` keyset index on the usage ledger concurrently in a no-transaction migration.
- `0132_usage_export_rollout_gate.sql` adds the durable `ai_usage_export_activated` rollout gate and the `worker_heartbeats.known_job_kinds` capability declaration.
- `0133_ai_usage_fact_ids.sql` installs the shared usage fact id namespace (audit, registry, triggers on both fact tables).
- `0134_ai_usage_export_audit.sql` extends the administrative audit shapes with `ai.usage_export_enabled` / `ai.usage_export_disabled` and the parked-job count of both disable commands. The checks are added `NOT VALID` and validated separately in 0139.
- `0138_ai_usage_job_fence.sql` adds the database claim-capability fence and export enqueue/requeue guard.
- `0139_ai_usage_export_audit_validate.sql` validates both widened audit checks and the expanded job-state check.
- `0140_ai_usage_capability_wait_index.sql` adds a concurrent partial claim index for protected pending jobs. Numbers 0135–0137 remain allocated to 09c.

The ledger does not cascade with projects and retains workspace identity directly. No export outbox table is added: the 04a `outbox_events` row keyed by the fact id is the export event and the emission record, and settlement writes no export row — export emits by replaying retained facts.
