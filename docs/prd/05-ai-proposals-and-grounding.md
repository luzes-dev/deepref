# AI proposals and grounding

## Goal

Use models to help reviewers without allowing model output to become
scientific state by itself.

## Requirements

- Model, prompt, schema, and input hashes identify every AI run.
- Structured output is schema-validated before semantic validation.
- Retrieval is project-scoped and can cite exact document blocks and pages.
- Screening, deduplication, grouping, appraisal, and extraction suggestions
  become typed proposals.
- Proposals include rationale, uncertainty, evidence references, and expected
  revisions where applicable.
- Reviewers can accept, edit, or reject proposals.
- Stale proposals remain pending and do not mutate state.
- Prompt injection stays inside the untrusted evidence envelope.
- Policy checks reject forbidden tools, cross-project access, arbitrary SQL,
  and direct scientific writes.

## Invariants

- AI cannot bypass the domain command or policy engine.
- Human-approved AI actions remain distinguishable from manual actions.
- Changing a protocol, model, prompt, or evidence input invalidates only the
  affected cached result.

## Acceptance evidence

- Gateway and policy code: `crates/ai/src/`.
- Proposal persistence and routes: `crates/postgres/src/ai.rs` and
  `crates/http-api/src/routes/ai.rs`.
- Regression suites:
  `crates/ai/tests/eval_gate.rs` and
  `crates/ai/tests/prompt_injection_regressions.rs`.
- Run:

```text
cargo test -p deepref-ai --test eval_gate --locked
cargo test -p deepref-ai --test prompt_injection_regressions --locked
cargo test -p deepref-ai --locked
```
