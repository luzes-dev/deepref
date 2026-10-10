# Assistant durability runbook (durable runs + cassette replay)

Operator notes for Level A durable assistant turns (Rig 0.44 + rig-cassette
migration, PR0–PR6). Execution belongs to the worker; the browser only
observes. See ADR 0006
(`docs/adr/0006-deferred-mid-turn-checkpoint-resume.md`) for what durability
covers and what it deliberately does not.

## Disconnect — the assistant keeps working

Browser disconnect, navigation, or Stop **never cancels work or spend**. The
worker claims the durable job and drives the Rig turn to settlement; the
browser observes persisted run events and never owns execution
(`services/worker/src/assistant.rs:1-12`,
`chat_durable` in `crates/http-api/src/routes/assistant/runs.rs`,
`stream_run_events` in
`crates/http-api/src/routes/assistant/run_events.rs`). Closing the tab only
drops the local SSE observer; reconnecting with `after_seq` replays what was
missed.

There is no cancel path for assistant runs: no route transitions a run to
`cancelled`, and `suspended` was removed entirely (migration
`0058_assistant_runs_no_suspended.sql`). `cancelled` still exists in the
column CHECK and parses in `AssistantAgentRunStatus`
(`crates/postgres/src/assistant_runs.rs`), but a tree-wide search surfaces
no writer — it is parse-only today. (The web
Svelte copy for this state is owned by the web agent; it must use this same
wording: "Disconnect — the assistant keeps working".)

## Lease recovery semantics

- Jobs are claimed with a lease (`WORKER_CLAIM_LEASE_SECS`, default 60s in
  `services/worker/src/config.rs:21-33`); expired leases are requeued by
  `recover_expired_jobs` (`crates/postgres/src/jobs.rs:60-67`) and each claim
  increments `attempts` (`jobs.rs:12-28`).
- `begin_assistant_agent_run` takes over a `queued` or `running` run after
  recovery and **continues the append-only event log** — prior events are
  preserved with monotonic seqs (guarded by `PRIMARY KEY (run_id, seq)` from
  migration 0057), and the redrive inserts one `status` boundary event
  ("Retrying after interruption") so observers can see the seam. Every SSE
  frame carries its persisted seq as the `id:` field; observers resume with
  `after_seq`, never by counting frames
  (`crates/postgres/src/assistant_runs.rs`,
  `crates/http-api/src/routes/assistant/run_events.rs`).
- Terminal runs are never re-executed
  (`begin_assistant_agent_run`, `assistant_runs.rs:359-380`); a
  duplicate delivery after completion is acked as a no-op
  (`services/worker/src/assistant.rs:90-100`).
- Completion is idempotent under the run's fixed answer/plan ids: one run
  yields at most one final answer and one plan
  (`complete_assistant_agent_run`, `assistant_runs.rs:456-487`).
- What does **not** survive: worker death mid-turn loses in-flight progress
  (model steps, tool results, unflushed events). The retry re-runs the full
  turn and re-spends those calls — there is no resume-from-step-N (ADR 0006,
  "Consequences" / "What does NOT survive").

## Attempts and retry budgets (spend bounds)

- Job queue: `max_attempts` defaults to 5 (`crates/postgres/src/jobs.rs:220-235`).
  `fail_job` parks the job `dead` once `attempts >= max_attempts`, else
  requeues it (`jobs.rs:153-185`).
- Run level (`services/worker/src/assistant.rs`): attempts exhausted fails the
  run closed as `ai_attempts_exhausted` and terminates   (`fail_run`,
  `~106-125`, `~255-265`). Only gateway/persistence errors are retryable
  (`AiError::Gateway | AiError::Persistence`, `~276-282`), requeued with a
  30s `RETRY_DELAY` (`const RETRY_DELAY`, `~46`, `~113-125`). Budget
  exhaustion fails closed as `ai_budget_exceeded` (`~250-258`); anything else
  fails closed as `ai_run_failed` (`~265-275`).
- Per-turn caps: `AgentLoopConfig::default()` allows 8 steps, 60,000 total
  tokens, 2,048 output tokens per call
  (`crates/ai/src/runtime/plan.rs:62-78`).
- Monthly budget is enforced twice: at submit, `chat_durable` returns 409
  `ai_budget_exceeded` when exhausted
  (`crates/http-api/src/routes/assistant/runs.rs:80-89`), and mid-run
  `BudgetExceeded` fails the run with the same code
  (`services/worker/src/assistant.rs:~250-258`).
- Stored tool outputs are truncated past 4,000 chars
  (`stored_output`, `services/worker/src/assistant.rs:~372-380`); run
  failure codes (`ai_attempts_exhausted`, `ai_budget_exceeded`,
  `ai_run_failed`) are observable on the run's `error` field and as `error`
  SSE events (`fail_run`, `~283-296`).

## Where to observe runs

- `POST /projects/{project_id}/assistant/chat`: deterministic tool commands
  still stream synchronously (200 SSE); free-form turns return **202** with an
  `AssistantRunDto` and execute in the worker
  (`chat_durable` in `crates/http-api/src/routes/assistant/runs.rs).
- `GET /projects/{project_id}/assistant/runs/{run_id}` returns status plus
  `answer_message_id`, `plan_id`, `error`, and timestamps (`run_dto` /
  `get_run` in `crates/http-api/src/routes/assistant/runs.rs:41-131).
- `GET /projects/{project_id}/assistant/runs/{run_id}/events?after_seq=N`
  streams persisted events (`stream_run_events`,
  `crates/http-api/src/routes/assistant/run_events.rs:43-117`;
  `list_assistant_run_events`, `assistant_runs.rs:612+`). The worker polls
  in batches of 200 with a 500ms cadence (`run_events.rs:66-112`).
  Persisted kinds are `status, text, replace, tool_start, tool_complete,
  plan, done, error` (`0057_assistant_agent_runs.sql:39-46`; wire mapping in
  `run_event_frame`, `run_events.rs:152-248`).

## Cassette replay operator note

Replay executes tools **live** against current project state; only completion
dispatches are payload-checked from the log
(`crates/ai/src/runtime/cassette.rs:22-36`, `216-236`). Concretely:

- Model dispatches are answered by replayers registered only for the log's
  completion keys with `RequestCheck::Payload`; tool keys are deliberately
  **not** registered, so every tool call re-executes through the host
  (`register_model_replayers`, `cassette.rs:218-236`). The log must carry
  exactly one completion key or it is treated as corrupt (`completion_key`,
  `cassette.rs:198-214`).
- Any drift — tampered spec, changed tool catalog or hooks, exhausted log —
  fails closed via `check_compatible` / `check_replayable`
  (`cassette.rs:107-114`, `165`).
- Reads re-execute safely and proposals recreate `PlanAction` data without
  applying mutations; live tool execution during replay is deliberate so
  policy and validation paths stay covered (`cassette.rs:27-36`).

What this means for offline replay audits: a cassette is a regression /
debugging fixture, **not time travel**. Replaying an old log against current
project state can diverge (tool outcomes differ from recording time) and the
replay then fails closed instead of reproducing history. Deterministic audit
requires a deterministic host and unchanged state, not just the stored log.

Storage: cassettes persist in the `assistant_agent_runs.effect_log` column
(`0057_assistant_agent_runs.sql:24`), written once on successful completion
(`complete_assistant_agent_run`,
`crates/postgres/src/assistant_runs.rs:456-487`). Failed runs carry no
cassette (`AssistantCassette::assemble` runs "on success only",
`cassette.rs:79-105`).

**Gap, verified:** no read path surfaces cassettes. `get_assistant_agent_run`
SELECTs `effect_log` into `AssistantAgentRunRecord.effect_log`
(`assistant_runs.rs:78-119`, `326-357`), but a tree-wide search for
`.effect_log` consumers finds only the worker write path, unit tests, and
cassette internals — `AssistantRunDto` exposes no `effect_log` field
(`runs.rs:27-58`), the only `get_assistant_agent_run` callers are
`get_run` / `stream_run_events` / the worker claim check
(`runs.rs:124`; `run_events.rs:49, 99`; `services/worker/src/assistant.rs:90`), and
`replay_rig_turn` takes an in-memory `&AssistantCassette`
(`cassette.rs:136-139`) with no replay-from-run-id entry point. Auditing a
stored cassette today means reading the column out of band; confirm with the
owning team before promising operators a replay workflow.
