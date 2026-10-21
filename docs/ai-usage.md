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
3. Drain every N-1 API and worker that can dispatch paid AI. Wait for the configured quiet window; keep all legacy dispatchers stopped during the window. The enable command also requires no legacy ledger insert during that window, a complete backfill, and no AI job queued or running.
4. Enable the cap path with `deepref-server ai-usage enable-hard-caps --quiet-window-seconds <seconds> <operator-user-uuid>`. The action is audited. N paid AI admissions remain fail-closed until this command succeeds.
5. For rollback, first disable paid AI dispatch at the deployment level and drain N dispatchers. Then run `deepref-server ai-usage disable-hard-caps <operator-user-uuid>`; this audited command sets the gate false, so N admissions fail closed. Only after that may N-1 be rolled back in. Its writes remain counted by the legacy trigger.

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

Tracing records reservation acceptance and denial, reserved and settled amounts, actual cost, overage blocks, unresolved liabilities, and reconciliation counts without customer identifiers as metric labels. Provider-neutral export is deferred: Slice 09b will replay retained usage facts, so Slice 09a writes no export outbox rows.

## Migrations

- `0120_ai_usage_reservations.sql` expands the ledger, creates limits, monthly and lifetime buckets, reservations, block/admission and rollout state, backfill markers, and legacy-write accounting. It seeds parent caps once from existing child limits, creates project buckets only for workspace-assigned projects, and does not scan the full ledger.
- `0121_ai_usage_audit_expand.sql` and `0122_ai_usage_audit_validate.sql` add and validate audit shapes for limit changes, hard-cap rollout, and reservation resolution.
- `0123_ai_usage_indexes.sql`, `0125_ai_usage_job_fence_index.sql`, and `0126_ai_usage_ledger_reservation_index.sql` each build one concurrent index in a no-transaction migration.
- `0124_ai_usage_reconcile_job.sql` maps the workspace recovery job into the AI workload pool.
- There is no 0127 validation migration in this slice. The `NOT VALID` period-start check remains until the follow-up validation migration after production backfill.

The ledger does not cascade with projects and retains workspace identity directly. No export outbox table or settlement insert is added in this slice.
