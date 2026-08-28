# Automation and assistant

## Goal

Run repeatable project workflows through durable jobs and a constrained
assistant.

## Requirements

- Automation definitions declare triggers, steps, status, and project scope.
- Events can dispatch matching automation runs.
- Jobs use PostgreSQL leases, retries, idempotency keys, and owner fencing.
- Completed steps are not repeated after a worker crash or retry.
- Unknown steps fail closed.
- The assistant exposes a fixed catalog of read and proposal tools.
- Tool execution records actor, project, proposal, and usage context.
- Usage and run history are visible in the automation center.

## Invariants

- Automation retries do not duplicate scientific effects.
- A worker cannot complete a job it does not own.
- Assistant tools cannot directly write eligibility, appraisal, grouping, or
  extraction truth.

## Acceptance evidence

- Automation persistence and execution:
  `crates/postgres/src/automations.rs`,
  `services/worker/tests/automation.rs`, and
  `services/worker/tests/crash_boundaries.rs`.
- Assistant policy tests:
  `crates/ai/src/agent_tests.rs` and
  `crates/http-api/tests/assistant_postgres.rs`.
- Run:

```text
cargo test -p deepref-worker --test automation --locked
cargo test -p deepref-worker --test crash_boundaries --locked
cargo test -p deepref-ai --lib --locked
cargo test -p deepref-http-api --test assistant_postgres --locked
```
