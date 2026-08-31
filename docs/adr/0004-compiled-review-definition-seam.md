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
scientific authority. Automation-triggered consequential runs require an
immutable, passing calibration bundle whose semantic-bundle hash exactly
matches the compiled manifest. The small checked-in evaluation set remains a
CI regression fixture and cannot authorize production automation.

The seam fails closed. Missing assets or invalid workflow transitions prevent
definition compilation; stale subject or protocol revisions block
finalization; missing, failed, or stale calibration rejects automated
scheduling; screening disagreement requires human adjudication; and exhausted
semantic repair produces a terminal blocked state. Failed or running attempts
never reserve reuse, and finalization is unique by run and candidate hash.

## Consequences

- Prompt, schema, policy, workflow, parser, model, protocol, and runtime changes
  invalidate the appropriate semantic identity.
- Reviewer and automation callers cross the same interface and receive the
  same proposal semantics.
- Adding a consequential workflow requires a checked-in definition and typed
  subject variant rather than route-specific orchestration.
- The TypeScript workflow project is an architectural reference, not a runtime
  dependency or sidecar.
- Audit exports include manifests, immutable attempts, content-addressed
  lineage, calibration evidence, linked AI runs, proposals, and explicit final
  reviewer decisions while redacting raw model and proposal payloads.
