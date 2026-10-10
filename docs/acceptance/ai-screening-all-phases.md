# AI screening V1 implementation and acceptance roadmap

Status: **all seven selected V1 phases implemented and verified, 2026-10-09**.
The user expanded the earlier Phase 1 instruction to finish the screening
workflow. This completes the V1 below. It does not certify scientific
assumptions, establish true recall, or attest that reviewers are distinct people.

The maintained methodology is [ADR 0005](../adr/0005-ai-screening-authority-and-validation.md).
[Phase 1 acceptance](ai-screening-phase1.md) is the completed historical
foundation report, including its commands and commits. Its statement that no
AI-first workflow exists describes the Phase 1 boundary. This document tracks
the later work, with implementation references and verification evidence below.

## Scope and phase status

| Phase                                     | Required outcome                                                                                                                                                                                                                                        | Current evidence/status               |
| ----------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------- |
| 1. Provenance and identity                | Availability provenance, expected blinding, independent pair filtering, stage-scoped immutable calibration, typed refusals, semantic identity scheme 2, secret-safe endpoint and runtime provenance                                                     | Complete; see Phase 1 report          |
| 2. Advisory and authority boundaries      | Advisory second reviewer usable from record one without calibration; owner ceiling separate from evidence authority; full-text and human replacement unchanged; no screening `act` or unlocked `final_exclusion` task                                   | Complete; verification below          |
| 3. Reversible routing                     | Title/abstract AI-first cohort and quarantine disposition layer; eligible exclusions leave the ordinary queue without changing scientific screening state; include/maybe/ineligible output goes to humans                                               | Complete; verification below          |
| 4. Fixed audit and reference              | Freeze membership and completed human-stratum count; register review-wide level; unbiased seeded sample without replacement; two distinct blind human identities; include/maybe OR reference labels; one draw and one look                              | Complete; verification below          |
| 5. Approved finalization and recovery     | Exact conditional-reference test at τ=0.95 or 0.98; per-cohort residual-reference attestation; current identity/protocol check; ordinary versioned screening events for unsampled exclusions; failure, exposure and amendments return records to people | Complete; verification below          |
| 6. Product, reporting and reproducibility | Reviewable settings/forecast, blinded labeling, explicit result/approval, status and recovery; honest PRISMA accounting; immutable exports sufficient to reproduce decision without model calls                                                         | Complete; verification below          |
| 7. Integration and release evidence       | Fresh migrated DB, meaningful regression coverage, workspace quality gates, SQLx/OpenAPI/client parity, actual diff review and documented limitations                                                                                                   | Complete; fresh V1 verification below |

Global certification, full-text AI-first, human replacement, automatic standing
finalization and auto-include are deferred. They are not necessary to complete
this V1 and are not implied by an owner raising its routing ceiling. Human
replacement needs a separate joint-error study and accepted margin; full-text
and standing policies need separate analyses. A storage API accepting supplied
calibration metrics is not a product evaluator that certifies them.

## Scientific acceptance contract

The target is **conditional reference retention**, given the fixed records
found by humans in I ∪ U and the reference-positive records in frozen X. It
omits true relevant records the human strata missed. Two blind labels define a
reference, not scientific truth. Self-asserted actor IDs enforce distinct
identifiers, not distinct people. Blinding does not establish independent
human/AI errors. UI, methods text and exports must preserve these distinctions.

For frozen population N_x, sample n, observed reference-positive count k and
fixed retained reference count R, use:

```text
D0(k) = floor((R + k)/tau - R) + 1
p = Pr_Hypergeom(N_x, D0(k), n)[K <= k]
pass iff p <= alpha_j
alpha_j = 0.05 / 2^j, j = 1, 2, ...
```

Exact-label validity follows by confidence-bound inversion: for any fixed true
reference count D, false certification implies `Pr_D[K <= k] <= alpha_j`.
Review-wide nominal control follows from the union bound since
`sum_j alpha_j = 0.05`, provided each test is valid conditional on its preceding
history. Neither result controls true scientific recall with erroneous labels.
An owner attestation records awareness; it cannot restore that missing premise.

Persist a project-wide cohort ordinal before the random draw; never reset it
on protocol/model changes, cancellations, failures or retries. Reserved audits
do not reclaim levels. Prohibit repeated draws on the same failed population,
label-dependent seed choice, selective removal after sampling and sample-size
extension after reading labels. If exposing or voiding sampled records breaks
the frame/reference assumptions, invalidate rather than silently drop them.

Use include or maybe from either audit reviewer as reference-positive. Both
reviewers must be blind to the AI verdict and the other judgment. Apply the
same declared retention definition to R, freeze R after completing human
strata and pre-draw targeted moves, and do not retrospectively shrink counts
using full-text results. V1 conservatively advances audit reference-positive records as title/abstract
includes even when positivity comes from a raw maybe; preserve both raw
labels and count the additional full-text work. Ordinary human-stratum maybe
behavior remains separate. No audit maybe disappears through automation
exclusion.

Handle integer/floating-point threshold boundaries, the empty null D0>N_x,
a full census, empty frames, and undefined zero-positive recall explicitly.
V1 conservatively stores floor(50,000,000/2^j) billionths for ordinals 1–25;
further audits refuse rather than reset or round a level to zero. The exact
integer level, numerator and denominator establish authorization. A floating
CDF is diagnostic only. Report the plug-in reference-retention point estimate
as an estimate, not the confidence lower bound.

### Blind task interleaving

The draw also freezes `N_c = min(n, N_h)` randomly selected human-stratum
controls, using an independent domain of the stored SHA-256 seed. A separate
seed domain shuffles sample and controls into the default queue. Controls are
never included in X, k or the retention inference. Both task classes require
two raw labels; their submissions return the same masked revision and statuses.
No sample decision changes canonical state until the single evaluation, and
ordinary control labels preserve the frozen human reference. If either label
finds a positive on a previously human-excluded control, evaluation invalidates
the cohort before any statistical look, rescues that record as a human include,
and returns the remaining cohort to human screening. R is never retrospectively
changed to manufacture a passing result.

During auditing, every cohort member's prior decisions, revisions and history
are hidden on queue, graph, assistant, full-text queue and audit export reads.
Status filters use that masked view. Project activity, run inspection and audit
exports are withheld entirely during the blind audit; conditional artifact
lineage is also withheld while routing is active. Both public label progress and completion
include controls. Prior human-stratum reviewers and the routing approver cannot
label this audit; known cohort AI exposure also excludes an actor. Reviewers must
be fresh independent people who have not seen prior cohort judgments. Stored
IDs enforce these checks only for self-asserted identities; they cannot establish
who used an account or what was seen elsewhere. With no human-stratum controls,
a positive-size blind draw refuses and the owner must recover to ordinary human
screening. Editing full-text or undoing decisions while the blind audit is active
also requires recovery. After evaluation, ordinary reference edits invalidate
passing/finalized authority as before.

Controls add `2N_c` judgments on both success and fallback. Downstream full text,
targeted review and administration remain additional costs. Historical workload
tables omit these new control labels and remain illustrations, not V1 forecasts.

## Planner and workload acceptance

V1 computes the minimum n that would pass with k=0, at the registered alpha_j
and frozen R, N_x and tau. It is a **zero-miss minimum**, not an 80%-power
plan. The owner may select a larger n before drawing. Any observed findings
are evaluated at that same fixed n; failure remains possible.

Show actual parameters and workload before approval. For single screening of
human strata with dual-reference audit labels, successful finalization costs
`N_h + 2(n + N_c)` judgments. If remaining X returns to one human without duplicating
completed audit work, fallback costs `N + n + 2N_c`. Account separately for targeted
review, administration, downstream full text and AI calls. A recommendation
of at least 20% and 500 judgments saved must be labelled conditional on audit
success and compared with the project's actual single/dual-human baseline.
The economic threshold never supplies scientific permission.

[Stored workload tables](../adr/0005/results.md) reproduce historical
standalone-alpha, 80%-scenario-power illustrations. They are not operational
V1 forecasts. Their small imperfect-reference false-pass numbers are not
uniform guarantees. A first-cohort example at alpha_1=0.025, R=10, N_x=1000,
D=1 and n=975 has false true-recall certification 0.1225 if relevant audit
records are detected with sensitivity 0.90; the arithmetic is
`0.025 + 0.975 * 0.10`. This illustrates why true recall must not be claimed.

## Required behavioral evidence

| Area            | Acceptance examples                                                                                                                                                                                                                                                                                                |
| --------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Scope           | Full-text cohort creation and unsupported authority levels rejected; advisory requests still do not write scientific state                                                                                                                                                                                         |
| Eligibility     | Declared versioned structural/evidence rule governs quarantine; self-reported model confidence never grants authority; missing/short abstracts and unresolved output remain with humans                                                                                                                            |
| Routing         | Quarantine changes queue visibility and counters only; disabling/voiding returns records without creating an exclusion event                                                                                                                                                                                       |
| Sampling        | Stable stored frame, unbiased recorded algorithm and seed, fixed n and frozen R; draw cannot be repeated or changed after labels                                                                                                                                                                                   |
| Reference       | Same actor cannot provide both labels; neither reviewer sees verdict or peer label before submission; include/maybe OR rescues; exposure invalidates inference                                                                                                                                                     |
| Statistics      | Exact BigUint authority comparison with independent small-population oracle, rational cutoff ties, diagnostic float separation, tau 0.95/0.98, zero-positive/empty/census behavior, alpha allocation under concurrency, conservative integer levels and ordinal exhaustion, no budget reset, single-look rejection |
| Finalization    | Needs passing result, current identity/protocol, frozen frame/reference validation and explicit residual attestation; newer human events win; atomic lifecycle/event links prevent partial orphaned finalization                                                                                                   |
| Recovery        | Audit failure, amendment, identity mismatch, reference exposure, human undo and deletion of frozen members invalidate explicitly; sampled/targeted rescues cannot be silently excluded; budget stop exposes partial coverage                                                                                       |
| Reporting       | Only actual automation-authored finalized exclusions count as PRISMA automation exclusions; quarantine, voided and human-audited records classified separately                                                                                                                                                     |
| Reproducibility | Export contains frame/membership, ordinal/level, seed/algorithm, fixed n/R, blinded reference provenance, result/policy/identity, attestation/approval and event links; authority can be recomputed without LLM rerun                                                                                              |

## Implementation map

- [Blind controls migration](../../crates/postgres/migrations/0055_ai_first_blind_controls.sql): frozen control membership, fresh-reviewer admission and masked/interleaved queue policy.
- [Migration 0054](../../crates/postgres/migrations/0054_ai_first_cohorts.sql): separate owner ceiling, immutable dispositions, frozen frame and dual reference labels.
- [Cohort lifecycle](../../crates/postgres/src/ai_first/cohorts.rs), [routing](../../crates/postgres/src/ai_first/routing.rs), and [audit/finalization](../../crates/postgres/src/ai_first/audit.rs): approvals, bounded worker scheduling, current-evidence checks and recovery.
- [Pure audit calculation](../../crates/application/src/ai_first.rs) and [eligibility rule](../../crates/review/src/ai_first.rs): exact integer authority and grounded two-screen agreement.
- [API](../../crates/http-api/src/routes/ai_first.rs) and [UI](../../apps/web/src/lib/features/ai-autonomy/components/AiFirstPanel.svelte): fixed-sample workload forecast, blinded actor attribution, explicit approval and recovery.
- [Lifecycle regressions](../../crates/postgres/tests/ai_first.rs) and [compiled worker regression](../../services/worker/tests/ai_first.rs): real event writes, invalidation, ceiling checks, exposure, model changes, later imports and budget stops.
- [Audit export](../../crates/postgres/src/audit_export.rs) and [independent reproduction tool](../../scripts/reproduce-ai-first-audit.py): terminal evidence without model reruns.

Recovery withdraws the current owner ceiling when no separately approved active
cohort remains. Previously drawn failed or invalidated frames return to humans;
a fresh start cannot recycle their still-unscreened records into a redraw.
Full-text decisions do not change a frozen title/abstract reference. Changing
that title/abstract reference or its underlying report text invalidates it.
Transactions fence model-route writes, source edits and member deletion through
the final evidence check and commit. Terminal exports preserve historical design;
active exports hide conditional model-call counts and topology as well as verdicts.

The UI forecast recomputes successful/fallback workload and advice for the
owner's selected larger sample, not just the zero-miss minimum. The UI also reports completed AI runs against the frozen intake; partial
coverage does not hide unevaluated records from humans. Forecasts are
available after closing the routing cohort and exclude administration, model
cost and downstream full-text work. They compare with single human screening;
no dual-human replacement claim is made.

## Reproduce a completed audit

Download the project's audit CSV, then run:

```bash
python3 -I scripts/reproduce-ai-first-audit.py audit.csv --cohort COHORT_UUID
python3 -I scripts/reproduce-ai-first-audit.py --self-test
```

The standard-library tool verifies the frozen frame, seeded sampling algorithm,
distinct actor labels, OR retention rule, R, ordinal and exact integer alpha,
and stored inference. It prints the exact rational CDF and distinguishes a
historical pass from a currently finalized cohort. It does not certify identities,
label accuracy, current report state or scientific recall. Active blinded evidence
is withheld from the export until the cohort becomes terminal.

## Final verification record

These commands were run after V1 integration. The final Rust run used the newly
created PostgreSQL 17 database `deepref_codex_release_verify20261009` on local
port 5433, with migrations through 0055 applied from scratch. `DATABASE_URL`
was set for all database tests, `SQLX_OFFLINE=true` for Rust compilation, and
`NEXTEST_TEST_THREADS=1` to isolate global model-route fixtures. Main remained
unchanged; implementation is on `feature/ai-screening-phase1` in the Claude
worktree.

The integrated diff review covered the authority transitions, screening writes,
worker artifact consumption, exact inference, exposure/read boundaries,
generated contracts and UI. Fixes included the real compiled-artifact join,
project locking and atomic event links, masked prior reviewer opinions and
full-text proposals, hidden artifact lineage, stable mixed queue pagination,
control-positive recovery, composite-reference provenance and partial AI
coverage reporting. The lifecycle and worker regressions exercise these paths.

Reviewer identity remains self-asserted, and provider aliases may change without
an observable immutable model revision. Software checks do not validate the
human reference or establish scientific recall. Verification uses disposable
database fixtures and a deterministic fake model gateway; it does not screen
production records or establish live-provider accuracy. The three explicitly
ignored Rust tests require external network access (Crossref abstracts and
Unpaywall) or running GROBID, PDFium and a sample PDF.

| Gate                                                               | Status                                                                                                                                                               |
| ------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Fresh migration application and V1 integration regression coverage | Passed: 824 Rust tests, three explicitly ignored external-service tests; all nine AI-first lifecycle tests and the compiled-worker regression passed                 |
| Rust workspace check/clippy/format/boundaries                      | Passed: workspace/all-target check, Clippy with warnings denied, doctests, rustfmt, and all 18 workspace architecture boundaries                                     |
| SQLx preparation/check and OpenAPI/client generation parity        | Passed: fresh migration/metadata preparation, metadata check, ten golden checks, generated contracts and final parity check                                          |
| `pnpm check`                                                       | Passed: zero errors; one existing `ScreeningTable` initial-value warning                                                                                             |
| `pnpm test`                                                        | Passed: 545 tests (26 UI, 519 web), including 12 AI-first panel tests                                                                                                |
| `pnpm lint`                                                        | Passed: zero errors; 63 existing ESLint warnings                                                                                                                     |
| Workload/reference computation and documentation links             | Passed: 14 historical workload checks, 3,479 exact-label configurations, six reproduction self-checks, five database-backed terminal audits, and documentation links |
| Actual integrated diff review, issue fixes and affected reruns     | Passed: fixes above included in the final full Rust and frontend runs; `git diff --check` clean                                                                      |

Commands run during final integration:

```bash
cargo xtask sqlx prepare
cargo xtask sqlx check
DEEPREF_BLESS_GOLDEN=1 cargo test -p deepref-review golden
cargo nextest run --workspace --tests --locked --no-fail-fast
cargo test --doc --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo check --workspace --all-targets --locked
cargo xtask boundaries
cargo xtask generate
cargo xtask generate --check
cargo fmt --all -- --check
pnpm check
pnpm test
pnpm lint
python3 -I docs/adr/0005/workload.py
python3 -I scripts/reproduce-ai-first-audit.py --self-test
python3 -I scripts/reproduce-ai-first-audit.py /tmp/deepref-v1-release-audit.csv
bash scripts/check-docs.sh
git diff --check
```

The terminal CSV reproduction used the same `to_jsonb(c)` and `to_jsonb(l)`
payloads as the public export. It independently verified all five evaluated
integration fixtures: four historical passes (`n=34`, 20 controls, `k=0`, exact
CDF `1/52`) and one failure (`k=1`, exact CDF `53/988`), at first-cohort level
0.025. The four passing fixtures had subsequently been invalidated by the
recovery regressions; reproduction preserves their historical result and does
not restore authority. Public-export visibility and payload inclusion are also
asserted by the lifecycle and compiled-worker tests.
