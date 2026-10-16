# Automation and assistant

## Goal

Run repeatable project workflows through durable jobs and a constrained
assistant.

## Requirements

- Automation definitions declare triggers, steps, status, and project scope.
- Events can dispatch matching automation runs.
- Jobs use PostgreSQL leases, retries, idempotency keys, and owner fencing.
- Completed steps are not repeated after a worker crash or retry.
- Recipe lifecycle lives in canonical durable runs, steps, and attempts; typed
  automation detail records retain recipe snapshots without a second lifecycle.
- Each recipe step executes through one `durable_run_step` job. Completion
  atomically schedules its successor; workers do not loop over an entire run.
- Unknown steps fail closed.
- The assistant exposes a fixed catalog of read and proposal tools.
- Tool execution records actor, project, proposal, and usage context.
- Usage and run history are visible in the automation center.

## Invariants

- Automation retries do not duplicate scientific effects.
- A worker cannot complete a job it does not own.
- Retrying a failed recipe preserves the run UUID, its first job UUID, and the
  failed step's existing job UUID; completed predecessor steps are not reclaimed.
- Step claims and finishes require the live job lease's UUID, owner, and attempt.
- Assistant tools cannot directly write eligibility, appraisal, grouping, or
  extraction truth.

## Acceptance evidence

- Automation persistence and execution:
  `crates/app/src/automations.rs`,
  `services/worker/tests/automation.rs`, and
  `services/worker/tests/crash_boundaries.rs`.
- Assistant policy tests:
  `crates/ai/src/agent_tests.rs` and
  `crates/http-api/tests/assistant_postgres.rs`.
- Run:

```text
cargo test -p deepref-app --test automations --locked
cargo test -p deepref-worker --test automation --locked
cargo test -p deepref-worker --test crash_boundaries --locked
cargo test -p deepref-ai --lib --locked
cargo test -p deepref-http-api --test assistant_postgres --locked
```
