# Evaluation, audit, and operations

## Goal

Keep the system reproducible in CI and make operational failures visible
without weakening scientific guarantees.

## Requirements

- Gold-set evaluation gates measure screening sensitivity, specificity,
  abstention, false negatives, weighted loss, numeric extraction, and citation
  correctness.
- Prompt-injection and forbidden-tool regressions run in CI.
- Every public operation has a unique OpenAPI operation ID.
- Generated OpenAPI and Orval clients have a drift check.
- Rust, web, documentation, container, and infrastructure checks run from
  pinned, reproducible inputs.
- Health endpoints distinguish core readiness from degraded dependencies.
- Audit exports cover long-term review history, model runs, proposals,
  decisions, protocol versions, and provenance.
- Production acceptance distinguishes local verification from hosted apply and
  recovery drills.

## Invariants

- A model change cannot pass CI by hiding false exclusions or citation errors.
- Audit exports do not rewrite or collapse historical events.
- Missing hosted evidence is reported as pending, not claimed as verified.

## Acceptance evidence

- Evaluation and security: `docs/acceptance/pr15-evals-hardening.md`,
  `crates/ai/tests/`.
- API and generation contracts:
  `crates/http-api/src/routes/mod.rs` and `scripts/check-api-codegen.sh`.
- Operational acceptance:
  `docs/acceptance/production-platform.md` and
  `infra/tests/static-contracts.sh`.
- Run:

```text
cargo test --workspace --all-targets --locked
pnpm run generate:api:check
bash scripts/check-docs.sh
bash infra/tests/static-contracts.sh
```

Hosted deployment, recovery, and admission drills remain operational work.
Local source and test evidence cannot close those requirements.
