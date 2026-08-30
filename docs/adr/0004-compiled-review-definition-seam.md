# ADR 0004: Compile review semantics before execution

## Status

Accepted.

## Context

Consequential AI tasks originally selected prompts, schemas, validation, and
proposal behavior in HTTP handlers. Durable automation would otherwise need to
reproduce those choices and could silently diverge from reviewer-requested
execution. Prompt version labels alone also did not identify workflow, policy,
parser, or runtime changes.

## Decision

Add `deepref-review` as the seam between callers and `deepref-ai`. A caller
selects a closed `ReviewDefinitionKey` and a typed `ReviewSubject`. The module
loads only checked-in assets, validates a closed workflow graph, hashes actual
asset contents, and builds the run manifest and node fingerprints.

`deepref-ai` remains the provider-neutral implementation of one structured
model call. `deepref-postgres` and the worker remain adapters that own durable
state and execution. HTTP and assistant adapters never submit arbitrary
workflow graphs, prompt text, or scientific write tools.

The run origin distinguishes reviewer-requested execution from calibrated
automation. Both origins can create proposals only; calibration does not grant
scientific authority.

## Consequences

- Prompt, schema, policy, workflow, parser, model, protocol, and runtime changes
  invalidate the appropriate semantic identity.
- Reviewer and automation callers cross the same interface and receive the
  same proposal semantics.
- Adding a consequential workflow requires a checked-in definition and typed
  subject variant rather than route-specific orchestration.
- The TypeScript workflow project is an architectural reference, not a runtime
  dependency or sidecar.
