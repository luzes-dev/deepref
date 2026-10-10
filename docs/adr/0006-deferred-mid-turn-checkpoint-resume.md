# ADR 0006: Defer serialized mid-turn checkpoint/resume for assistant agent runs

## Status

Accepted.

## Context

The Rig 0.44 + rig-cassette migration (branch `feat/rig-044-cassette-migration`,
PR0–PR6) implements Level A durability for assistant turns:

- `crates/ai/src/runtime/agent.rs` runs one turn on Rig's `AgentRunner`;
  `run_rig_turn_channel` splits execution from observation so a slow or
  dropped observer never stalls or cancels the run.
- `crates/ai/src/runtime/cassette.rs` records each run's model and tool
  dispatches into an `EffectLog` stamped with program identity and run
  metadata (`AssistantCassette`); replay serves the log's completion keys
  offline while tools execute live, and any divergence fails closed.
- `crates/postgres/migrations/0057_assistant_agent_runs.sql` persists
  `assistant_agent_runs` (queued/running/completed/failed plus fixed
  answer/plan ids, effect log, run-spec hash) and `assistant_run_events`
  (seq-keyed, batched text, reconnectable SSE observation).
- `services/worker/src/assistant.rs` owns execution outside HTTP lifetime:
  the worker claims the durable job, runs the Rig turn, persists batched
  events, the effect log, the answer and the plan, then marks the run
  terminal. Transient provider errors requeue attempts-capped; budget or
  malformed runs fail closed; takeover plus idempotent completion keep one
  answer and one plan.

No repo requirement was found that demands surviving a worker process death
*mid-turn* with in-flight progress preserved. `docs/prd/06-automation-and-assistant.md`
requires leases, retries, idempotency, owner fencing, and that completed
automation *steps* are not repeated after a crash; that is step-level
(between-attempt) recovery, which Level A covers. The remaining `checkpoint`
and `pause/resume` hits in the docs refer to calibration audit checkpoints
and human-review pause/resume, not to serializing a running `AgentRun`.

## Decision

Defer Level B: serialized mid-turn checkpoint/resume of a running Rig
`AgentRun` (persisting partial turn state mid-execution and resuming it in a
new worker process). Lease recovery re-drives a running run from scratch;
fixed output identities plus conflict-safe writes keep the retry to one
final answer and one plan.

## Consequences

What survives today:

- Dropped SSE observers: execution continues to completion; the observer
  reconnects over persisted events.
- Lease expiry or crash between attempts: the recovered owner re-drives the
  run and idempotent completion keeps exactly one answer and one plan.
- Duplicate delivery: the second delivery is a no-op on the same fixed ids.
- Offline replay: a recorded effect log replays without a live provider;
  tampered specs, hooks, prompts, or exhausted logs fail closed.

What does NOT survive:

- Worker process death mid-turn loses in-flight progress (model steps, tool
  results, unflushed events not yet persisted). The job retries the whole
  turn from scratch, re-spending the model calls and tool executions already
  made. There is no resume-from-step-N.

## Reopen when

Revisit Level B only on a concrete trigger:

- Turns routinely exceed lease or retry budgets so from-scratch retry
  cannot converge, with measured waste (retry rate, duplicated cost).
- A product requirement explicitly demands surviving a mid-turn worker
  restart or deploy (e.g. rolling restarts during long turns) rather than
  between-attempt recovery.
- Retry-from-scratch cost becomes material and attributable: repeated
  full-turn re-execution tied to mid-turn interruptions, not to transient
  provider errors.
