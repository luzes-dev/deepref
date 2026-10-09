# AI screening Phase 1 completion

Scope: the architectural foundations and proposed ADR 0005 from the AI
Screening Autonomy handoff. This work grants no new scientific screening
authority. Screening autonomy remains `off`, `suggest`, or `second_reviewer`;
`final_exclusion` remains locked.

## Implementation

### Exposure and blinding

`ai_opinion_exposures` records project, report, stage, audience, opinion/proposal/run
links, source and earliest recorded availability. The audience is project-wide
unless a specific actor is named. Availability is sufficient; page-opening
telemetry is not required. Any prior AI opinion for the same record and stage
contaminates a later decision, even if it came from another model run.

Normal second-reviewer decisions, waiting proposals and activity entries stay
blind. Pending second-reviewer proposals are protected before the worker creates
their decision rows. Intentional workflow verdict routing remains available and
records exposure before returning its output. Database triggers capture suggestion
availability and post-decision reveals, including mode changes, before a page is
opened. Read paths record reveals conservatively as well.

`independent_reviewer_pairs` excludes exposed and unverifiable pairs. Only a
matching human decision event supports an independent label; an automation state
cannot be attributed to an older human event. Agreement and kappa use this set
without the old capped listing. The API reports excluded pairs. The same filtered
set is available to future calibration evaluators; the repository currently has
no product calibration-creation flow. Arbitrary externally supplied bundle
metrics are not independently certified by the storage API.

Conflict resolution writes screening, resolution provenance and exposure in one
transaction. Audit exports include exposure provenance without revealing a
waiting verdict.

### Calibration and identity

Title/abstract and full-text evidence have separate stages. Non-screening
bundles have null stage. Shared semantic changes can invalidate both stages;
stage-specific model route changes are isolated. Admission returns typed missing,
failed, stage-mismatch, incompatible-scheme or stale-component refusals.

Scheme 2 snapshots retain aggregate equality for admission and named components
for diagnosis: definition, prompt, schema, policy, parser, protocol, models,
provider endpoint, implementation and dependencies, plus golden render/parse for
screening. Unknown schemes cannot use the equality fast path.

Golden rendering covers persisted-subject transformation through actual request
construction. Golden parsing exercises raw response normalization, validation,
interpretation and existing routing helpers. Runtime-computed values establish
identity; committed snapshots make behavioral changes reviewable.

Screening uses a declared source boundary and semantic dependency closure,
including declared dependency features. The implementation boundary includes
availability/blinding policy. The broad source-tree hash remains audit provenance
and conservatively remains the implementation component for non-screening tasks.
Actual compiler version, build target and optional deployment id are recorded.

Provider endpoints retain scheme, host, nondefault port and path and discard
userinfo, query and fragment. Configuration must keep secrets out of paths.
Configured workers refuse mismatched or unpinned scheduler endpoints. Provider
response model and system fingerprint are stored for audit; mutable aliases can
still drift without a stable provider revision.

## Migrations and legacy data

| Migration | Behavior |
| --- | --- |
| 0049 | Append-only exposure records, conservative activity/proposal legacy backfill and pre-resolution human timestamp |
| 0050 | Stage and component snapshots; keeps ambiguous legacy bundles as scheme 1 with null stage/snapshot and refuses them for new admission; preserves immutability |
| 0051 | Provider-served model and system fingerprint on AI runs |
| 0052 | Availability tracking and protection for pending second-reviewer proposals |
| 0053 | Requires snapshot shape checks to be true; missing JSON keys cannot pass as SQL null |

Legacy evidence stays available for audit and requires recalibration. The
migration does not infer title/abstract or full-text provenance from an old
aggregate hash. Historical verdict availability is treated conservatively.
Exposure updates are rejected; project/report deletion can cascade normally.

## ADR and workload analysis

[ADR 0005](../adr/0005-ai-screening-authority-and-validation.md) remains Proposed.
It contains authority ceilings, evidence authority, promotions/demotions,
separate workflows, disposition design, eligibility policy, audit reference,
closed-cohort estimator, amendment/budget/race behavior, PRISMA reporting,
reproducibility, comparison tables and owner decisions.

[Workload results](../adr/0005/results.md) and the
[calculation specification](../adr/0005/SPEC.md) document all assumptions.
Reproduce them with:

```bash
python3 -I docs/adr/0005/workload.py
```

The script scans all integer sample sizes because power is non-monotone and
checks finite-population coverage on small cases. The forecasts assume successful
finalization and omit fallback screening and downstream/administrative costs;
they are scenario illustrations, not measured product savings.

The statistical claim is conditional automation recall under fixed correct
reference labels, not total true review recall. Imperfect audit labels weaken
coverage: the draft includes an exact small-cohort counterexample. Human
replacement requires a separate joint-error validation design. Those choices,
review-wide error control and audit-reference sensitivity require owner review
before any future scientific authority is activated. The older methodological
document is not superseded until ADR acceptance.

## Verification

The database recipes used a fresh `deepref_codex_phase1` database on the local
Postgres server (port 5433), with `DATABASE_URL` exported. Its migration ledger
reached version 53. Nextest integration execution was serialized with
`NEXTEST_TEST_THREADS=1` because existing fixtures share global model routes and
worker job queues. New exposure tests fail if the configured database cannot be
reached; they do not silently report success without it.

| Command | Result |
| --- | --- |
| `just test-unit` | 545 Rust tests passed, 2 skipped; doctests passed; 26 UI and 502 web tests passed |
| `just test-integration` | 804 tests passed, 3 skipped, using the real migrated database |
| Final Postgres/review library rerun | 97 passed after identity and provenance corrections |
| Final worker library rerun | 23 passed, including unpinned endpoint rejection |
| Final exposure integration rerun | 11 passed, including availability without fetching, automation-label rejection, audit export and malformed snapshot refusal |
| `cargo check --workspace --all-targets --locked` | Passed |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo xtask boundaries` | Passed for 18 workspace members |
| `just sqlx-prepare`, `just sqlx-check` | Passed; metadata is current |
| `just codegen`, `just codegen-check` | OpenAPI and Orval client regenerated; generated contracts verified current |
| `pnpm check` | Passed; existing ScreeningTable initial-value warning remains |
| `pnpm test` | 26 UI and 502 web tests passed |
| `pnpm lint` | Passed; existing 63 frontend warnings, 0 errors |
| Workload calculator | 14 self-checks passed; includes 3,479 exact coverage configurations; repeated results match byte for byte |
| `bash scripts/check-docs.sh`, `git diff --check` | Passed |

The initial full unit run preceded the last small hardening edits; the final
library, worker and exposure reruns cover those edits. No live provider model
calls or production database migrations were required.

## Commit breakdown

The completed work is on `feature/ai-screening-phase1` in
`.claude/worktrees/phase1`, based on `97e4690`:

| Commit | Review scope |
| --- | --- |
| `01ed7b5` | Named semantic identity contract |
| `89a2520` | Stage-scoped calibration, snapshots and typed refusals |
| `6a6c849` | Screening source/dependency boundary and runtime split |
| `53542d7` | Golden rendering/parsing and deterministic subject transformation |
| `cda7e60` | Secret-safe endpoint and provider response provenance |
| `585bfee` | Integrated admission, snapshot, endpoint and runtime hardening |
| `4e912e8` | Availability provenance, blinding, independent metrics, API contracts and regression tests |
| `b2d3cfb` | Proposed ADR and reproducible workload analysis |

This completion report is committed separately. The branch is local; `main`
remains unchanged and the original Claude worktrees remain available.
