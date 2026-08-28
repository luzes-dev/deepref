# PR12 — AI screening and duplicate-assistance acceptance

## Locally proven

- Screening analysis contracts carry protocol/version identity, ordered criterion judgments, explicit `Maybe` and `InsufficientEvidence` outcomes, and stage-appropriate evidence references.
- Full-text citations use the Postgres hybrid retrieval path scoped to the target project and report, and only returned active document blocks may be cited; title/abstract citations use report metadata hashes and do not fabricate PDF blocks.
- Duplicate assistance carries a candidate pair, decision, rationale, signals, and identity provenance. It remains a proposal; deterministic identifier resolution remains the authoritative dedupe path.
- Project-scoped AI proposals are tied to immutable runs and typed proposal projections. Reuse is idempotent, divergent content is rejected, evidence projections enforce report/document/block identity, and approval/rejection is actor-audited compare-and-set.
- Approval calls the existing screening or record-resolution command inside the proposal transaction. A command failure does not mark the proposal accepted.
- HTTP routes resolve provider/model/version from Postgres and call the injected provider-neutral gateway. A missing adapter or gateway failure returns service-unavailable and leaves a failed run without a proposal; the server does not fabricate model output.
- OpenAPI, generated Orval clients, bounded server-side proposal target filters, Svelte Query invalidation, and in-context review surfaces are included for title/abstract, full-text, and dedupe workflows.

## Deployment and model-evaluation concerns

This acceptance slice does not establish live-provider quality, calibration, retrieval recall, prompt-injection resilience under production corpora, latency, cost, or provider availability. Deployment must register a concrete gateway adapter for each enabled Postgres route; otherwise the explicit unavailable-provider response is expected. Representative blinded evaluation sets, human agreement analysis, monitoring, and controlled rollout are still required before enabling a hosted model route.
