# PR15 evaluation and hardening acceptance

This slice keeps model evaluation as a pure comparison over a manually reviewed
fixture. `crates/ai/tests/fixtures/evals/reviewed-small-v1.json` stores the
schema/version, reviewer metadata, baseline and candidate provider/model
identity, prompt versions, gold cases, and the checked-in gate thresholds.

## Screening semantics

For a relevant gold record, `include` is a true positive, `exclude` is the only
false negative, and `maybe` retains the record while counting as an abstention.
For an irrelevant gold record, `exclude` is a true negative, `include` is an
unnecessary inclusion, and `maybe` is retained and counted as an abstention.

The module computes:

- sensitivity = true positives / relevant records;
- specificity = true negatives / irrelevant records;
- false-negative rate = false negatives / relevant records;
- abstention rate = all `maybe` predictions / all screening records; and
- weighted screening loss = `20 * false exclusions + 1 * unnecessary inclusions`
  in the reviewed fixture. The input schema requires the false-exclusion
  weight to be at least ten times the unnecessary-inclusion weight.

Candidate screening metrics must satisfy the absolute thresholds and may not
increase false exclusions or weighted loss beyond the configured comparison
tolerances. Numeric extraction uses numbers only: a value passes when absolute
error is within the gold absolute tolerance or, for a nonzero gold value,
`abs(predicted - gold) / abs(gold)` is within the relative tolerance. A zero
gold value therefore needs an exact value or an absolute tolerance; strings are
never parsed as numbers.

Citation correctness is exact set equality over durable evidence ID and
lowercase SHA-256 pairs. Article text is not used for citation scoring.

## Required checks

The explicit CI job `AI PR15 evaluation and security gates` runs:

```text
cargo test -p deepref-ai --test eval_gate --locked
cargo test -p deepref-ai --test prompt_injection_regressions --locked
```

The regression suite also proves that hostile title/abstract/document content
stays in the untrusted evidence envelope, fabricated evidence identities and
hashes fail semantic validation, consequential tasks produce scientific
proposals rather than direct state commands, and arbitrary SQL, final
exclusion, unknown tools, and cross-project requests do not invoke the
executor.

## Calibration is production admission, not a CI fixture

The checked-in `reviewed-small-v1.json` set is deliberately too small to grant
automated scientific authority. It is only a regression oracle. Consequential
automation remains disabled until an expert-adjudicated calibration bundle is
persisted for the project and exact compiled semantic-bundle hash. Admission
rejects a missing bundle, a failed bundle, or any stale hash after a change to
the definition, workflow, protocol, prompt, schema, policy, parser, model route,
or runtime build. Reviewer-requested proposal assistance does not require that
bundle and still cannot write scientific state directly.
