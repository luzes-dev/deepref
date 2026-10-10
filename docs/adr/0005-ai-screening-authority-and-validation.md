# ADR 0005: AI screening authority and validation

## Status

Selected for V1 implementation following the user's expanded instruction to
finish the workflow. All seven selected V1 phases are implemented and verified
as of 2026-10-09; [the acceptance report](../acceptance/ai-screening-all-phases.md)
records the implementation and quality gates. This status does not assert that the owner has
reviewed or scientifically validated the reference-quality assumptions.

V1 is title/abstract only, with advisory opinions from record one, reversible
quarantine, a closed fixed audit with two blinded human reference labels,
review-wide alpha spending, and explicit per-cohort owner attestation before
finalization. Human replacement, full-text AI-first, standing finalization and
global certification remain outside V1. Descriptions of the selected V1 below
document the verified implementation; future alternatives remain explicitly deferred.

The historical document is marked as superseded where V1 software behavior
changes its calibration prerequisite. Scientific assumptions and their
limitations remain explicit; expanded implementation authorization does not
certify them. This maintained implementation decision:

- supersedes [`docs/decisions/ai-screening-calibration.html`](../decisions/ai-screening-calibration.html),
  which now carries a "Superseded by ADR 0005" banner;
- amends [ADR 0004](0004-compiled-review-definition-seam.md) as described in
  [section 2.6](#26-what-changes-in-adr-0004-and-in-project_ai_autonomy).

## Context

At Phase 1 completion, DeepRef's AI could screen records but had no scientific
authority; screening decisions were human events. V1 changes the separate
cohort-finalization path described below. The AI's opinion is
either a suggestion (`suggest`) or an independent second opinion stored beside
the human decision (`second_reviewer`). `project_ai_autonomy` forbids `act` for
both screening tasks, and `final_exclusion` is a locked task that cannot even
be stored. At that boundary, automation-triggered review runs needed an immutable, passing
calibration bundle whose semantic identity matches the compiled review
(ADR 0004). There is no product calibration-bundle creation flow. At that Phase 1 boundary, an automatic second reviewer without a compatible
bundle remained paused. V1 removes the calibration prerequisite for advisory
opinions; legacy consequential admission keeps its stage-specific evidence gate.

The owner wants more AI autonomy from the start of a review without giving up
validity, auditability, reversibility or explicit human control. The earlier
decision document proposed one gate for one workflow. It set 60 true includes,
300 records, a 95% point sensitivity and a 90% one-sided lower bound, all for
the advisory second reviewer. It never covered AI-first screening, an
imperfect reference standard, workload economics, or reviewer exposure to AI
verdicts, which the product shows on purpose in several places.

Phase 1 (section 20) added the foundations this ADR depends on:

- durable exposure provenance;
- stage-scoped calibration evidence;
- decomposable semantic identity snapshots with typed refusal reasons;
- identity scheme 2: a narrow implementation boundary, golden render and parse
  fingerprints, a dependency-closure fingerprint, and a secret-safe provider
  endpoint.

## Decision summary

| Question                         | Recommended answer (section)                                                                                                                                                                                                                                                                    |
| -------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Authority state model            | Two ladders, one per workflow: A (human screens everything) and B (AI-first). Effective authority = min(owner ceiling, evidence-backed authority). Promotion needs explicit owner approval; demotion is automatic. (2)                                                                          |
| Owner ceiling vs evidence        | The owner ceiling is stored per project, stage and workflow. Evidence authority is computed from stored observations and the current semantic identity. Evidence never raises authority above the ceiling. (2.1)                                                                                |
| Second-reviewer semantics        | The existing `second_reviewer` level means **advisory**. Replacing a human second screener (`dual_replacement`) is deferred pending a joint-error validation design; V1 remains advisory. (3)                                                                                                   |
| AI-first disposition model       | A separate `ai_screening_dispositions` routing layer. A quarantined record stays scientifically unscreened. Recovery voids the disposition. (4)                                                                                                                                                 |
| Automation-eligible exclusion    | A versioned, deterministic rule over the structured output. It never uses self-reported confidence. It is part of semantic identity. (5)                                                                                                                                                        |
| Random audit design              | A closed title/abstract cohort. Freeze the frame and human reference count, choose the zero-miss minimum sample (or enlarge before drawing), and draw an unbiased seeded simple random sample without replacement. Two distinct blinded human labels; targeted review remains separate. (6–8)   |
| Reference standard               | Hybrid: two blinded independent reviewers with an OR rule at title/abstract. Disagreements are not adjudicated for the estimator. Audit includes continue to full text. (8)                                                                                                                     |
| Cohort finalization rule         | A one-sided exact hypergeometric bound on conditional reference retention ≥ τ = 0.95 or 0.98, using α_j = 0.05/2^j for project-wide cohort ordinal j ≥ 1; correct fixed reference labels assumed, one draw and one look. It does not bound total review recall with an imperfect reference. (7) |
| Promotion / demotion             | Promotion: owner approval for every increase, and for every cohort finalization in V1. Demotion: automatic on identity change, protocol republish, audit failure, exposure of reference labels, drift signal, or budget stop. (2.4)                                                             |
| Protocol changes                 | V1 is conservative. Open dispositions are voided, open samples discarded, authority reset. Automation-finalized exclusions are reopened when the amendment touches that stage's criteria. (13)                                                                                                  |
| Budget exhaustion                | Seeded random evaluation order. The cohort closes on the covered subset. Uncovered records stay in the human queue, and coverage is explicit. (14)                                                                                                                                              |
| Race safety                      | Finalization writes ordinary screening events with the evaluated `expected_revision`. Any human event on the record voids the disposition. (15)                                                                                                                                                 |
| PRISMA                           | Only cohort-finalized exclusions count as "records marked as ineligible by automation tools". Quarantined records count as awaiting screening. (16)                                                                                                                                             |
| Reproducibility                  | The authority decision is recomputable from stored evidence without an LLM call. (17)                                                                                                                                                                                                           |
| Break-even economics             | Total human judgments are compared, including dual audit labels. AI-first pays off only for large title/abstract stages with many relevant records. (9)                                                                                                                                         |
| Product recommendation threshold | Recommend AI-first only when it is statistically achievable, saves ≥ 20% **and** ≥ 500 judgments against the comparable baseline. Otherwise say "allowed, but unlikely to reduce workload". (10)                                                                                                |

## 1. Two screening workflows, kept separate

DeepRef supports two workflows that make different methodological claims.
They are separately configurable, separately recorded on every AI run and
disposition, and separately reported. Neither is a "higher level" of the other.

**Workflow A — human screens every record.**
A person screens every record at the stage. The AI contributes an opinion:

- `suggest`: the opinion is shown while the person screens. This is
  recommendation only, and exposure is intended.
- `second_reviewer` (advisory): the opinion is recorded blind and revealed
  only after the person decides. Disagreements surface as conflicts.
- `dual_replacement` (deferred): a future human + AI replacement methodology
  would require accepted joint-error validation. V1 provides no such autonomy
  setting and never counts an advisory AI opinion as a second human judgment.

**Workflow B — AI-assisted first pass (AI-first).**
The AI evaluates records first. Records whose output satisfies the
automation-eligible exclusion rule (section 5) are routed out of the normal
human queue into a quarantine disposition. All other records go to people. A
probability sample of quarantined records is audited. Only a cohort that
passes the statistical rule and receives owner approval is finalized as
excluded through ordinary screening events.

What a methods section may claim differs by workflow:

| Workflow                       | Claim a methods section may make                                                                                                                                                                                                                      |
| ------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| A / suggest                    | "Screened by one (or two) reviewers with AI suggestions visible."                                                                                                                                                                                     |
| A / second_reviewer (advisory) | "Screened by one reviewer. An AI tool gave a blinded independent opinion. Disagreements were reviewed." It never claims dual screening.                                                                                                               |
| A / dual_replacement           | Deferred until a project-specific joint-error comparison validates replacement against dual human screening. Marginal AI sensitivity alone cannot justify this claim.                                                                                 |
| B / AI-first                   | "AI-assisted screening with a fixed probability audit of automation exclusions (conditional automation recall bound relative to the stated reference rule; closed cohorts; n audited …)." Report reference error and human-stratum losses separately. |

Workflow B is proposed for title/abstract only in V1. Section 9 models
title/abstract scenarios only. Small cohorts and few found relevant records
can make audits expensive at either stage; the tables do not prove that
full-text automation can never save work. Full text keeps workflow A pending
a separate stage-specific analysis.

## 2. Authority state model

### 2.1 Owner ceiling, evidence-backed authority, effective authority

For each project and screening stage there are two independent limits.

- **Owner autonomy ceiling.** This is what the researcher has chosen to allow.
  It is stored and changed only by an explicit owner action, with the actor
  and time recorded. It has one value per workflow ladder (below).
- **Evidence-backed authority.** This is what project-specific evidence
  currently justifies. It is never stored as a mutable flag. It is a pure
  function of immutable observations:
  - calibration bundles;
  - audit samples and blinded labels;
  - cohort results;
  - exposure provenance;
  - the current semantic identity and protocol version.
    Anyone can recompute it (section 17).

**Effective authority = the lower of the two** on the same ladder. Evidence can
never raise automation above the owner's choice:

| Owner ceiling          | Evidence-backed                          | Effective                                                |
| ---------------------- | ---------------------------------------- | -------------------------------------------------------- |
| B: cohort_finalization | B: routing (no passing cohort yet)       | routing                                                  |
| A: second_reviewer     | A: second_reviewer (current V1 identity) | second_reviewer                                          |
| B: routing             | none (identity changed)                  | none: AI-first paused, records return to the human queue |

### 2.2 States

**Ladder A (human screens every record):**

| State                         | Scientific effect                        | Evidence required                                                                                                                   |
| ----------------------------- | ---------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `off`                         | none                                     | none                                                                                                                                |
| `suggest`                     | none; opinion visible while screening    | current semantic identity only                                                                                                      |
| `second_reviewer` (advisory)  | none; blinded opinion, conflicts surface | current semantic identity only (section 3.1)                                                                                        |
| `dual_replacement` (deferred) | Unavailable in V1                        | Requires a separate accepted joint-error non-inferiority design, margin and current stage-scoped replacement evidence (section 3.2) |

**Ladder B (AI-first; title/abstract only in V1):**

| State                                      | Scientific effect                                                                | Evidence required                                                                                                                                        |
| ------------------------------------------ | -------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `off`                                      | none                                                                             | none                                                                                                                                                     |
| `routing`                                  | reversible quarantine of automation-eligible exclusions; records stay unscreened | current identity; owner opt-in; open cohort pinned to that identity (section 4)                                                                          |
| `cohort_finalization`                      | a closed, passing cohort may be finalized as excluded, one approval per cohort   | cohort passes the section 7 test at its registered α_j under the pinned identity, current identity/protocol, and explicit residual-reference attestation |
| `standing_finalization` (deferred, not V1) | passing cohorts finalize without per-cohort approval                             | k consecutive passing cohorts plus continuous monitoring; a separate future ADR                                                                          |

A project chooses one workflow per stage. If ladder B is not `off`, the stage
runs AI-first. Ladder A still applies to the records people screen (those in
the include and unsure strata). It is limited to `off`, `suggest` or
`second_reviewer`, because those records are the human-reviewed part of an
AI-first design.

### 2.3 Transitions

```text
                       owner approval            owner approval (per cohort)
   off ──► routing ──────────────────► cohort_finalization ──────────────► finalize(cohort)
    ▲          │   ▲                          │
    │          │   └── automatic demotion ────┘
    └──────────┴──── automatic demotion (any trigger in 2.4)
```

- **Increases need explicit owner approval.** This covers raising a ceiling,
  entering `routing`, and finalizing each cohort in V1. Human replacement
  remains deferred. The approval records the actor, time, the evidence summary shown to
  them, the identity snapshot, the protocol version, and explicit attestation
  acknowledging residual reference error and the conditional nature of the
  bound. The attestation cannot make an imperfect reference scientifically
  exact. Actor identity is
  self-asserted today (`x-actor-id`), so the approval is an attestation, not
  authentication. Deployments that need non-repudiation must add
  authentication first. This is an open item (section 22).
- **Decreases are automatic.** They need no approval and never wait for a
  person.
- **There is no "ignore calibration and continue anyway" override in V1.** A
  stale or failed state can only be cleared with new evidence under the
  current identity.

### 2.4 Demotion triggers (automatic)

| Trigger                                  | Detected by                                                                                                       | Effect                                                                                                                                 |
| ---------------------------------------- | ----------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| Semantic identity change (any component) | admission compare (`IdentityComparison::Stale`, typed reasons)                                                    | evidence authority for the stage falls to the identity-only states; open cohorts close as `invalidated`; their dispositions are voided |
| Protocol republish                       | protocol version change                                                                                           | as above, for every stage; see section 13 for finalized exclusions                                                                     |
| Cohort audit failure                     | section 7 test fails                                                                                              | the cohort's dispositions are voided (records return to the human queue); `routing` is suspended until the owner re-approves           |
| Exposure of reference labels             | exposure provenance on an audit label before it was recorded                                                      | the cohort is invalidated; labels are never selectively dropped to preserve a pass                                                     |
| Drift signal                             | targeted-review or downstream discovery of a relevant record inside a finalized cohort, or a monitoring threshold | suspend `cohort_finalization`; the owner is notified; finalized cohorts are flagged for review                                         |
| Provider/endpoint change                 | `ProviderEndpoint` or `Models` component stale                                                                    | same as an identity change                                                                                                             |
| Budget stop                              | AI budget exhausted                                                                                               | no new dispositions; the open cohort closes on its frozen intake with partial AI coverage (section 14)                                 |
| Owner lowers ceiling                     | owner action                                                                                                      | immediate; open dispositions are voided unless the new ceiling still allows `routing`                                                  |

### 2.5 Interaction with `project_ai_autonomy` and locked tasks

- `project_ai_autonomy` stays the **ladder-A ceiling** for screening.
  `title_abstract_screening` and `full_text_screening` keep the levels `off`,
  `suggest` and `second_reviewer`. The CHECK that forbids `act` for screening
  **remains**: no screening autonomy level lets the AI write a screening
  decision per record.
- `dual_replacement` is deferred. V1 does not widen the existing screening
  autonomy CHECK to add it. No owner toggle can replace the second human
  without the separate validated methodology in section 3.2.
- Ladder B gets its own owner-ceiling table, `project_ai_screening_authority`
  (project, stage, ceiling, updated_by, updated_at, approval reference). It is
  not a `project_ai_autonomy` task. AI-first is a different workflow, not a
  higher autonomy level.
- `final_exclusion` **stays locked** as an autonomy task. A per-record
  autonomous exclusion remains impossible. Cohort finalization is a separate,
  explicitly approved, statistically gated **cohort** action. It writes
  ordinary screening events under an automation actor, with the cohort id and
  approval id in the event notes or payload.

### 2.6 What changes in ADR 0004 and in `project_ai_autonomy`

ADR 0004 decisions that **remain valid**:

- the compiled-definition seam;
- closed definitions and typed subjects;
- fail-closed compilation;
- immutable attempts;
- the rule that calibration evidence never itself grants scientific authority.
  Authority comes from the owner ceiling **and** evidence **and**, for
  finalization, a per-cohort approval;
- mandatory human adjudication of AI-vs-AI disagreement inside a run;
- redacted audit exports.

ADR 0004 decisions that are **amended**:

1. "Automation-triggered consequential runs require an immutable, passing
   calibration bundle." Proposed `dual_replacement` still needs a replacement
   bundle. AI-first `routing` instead needs identity plus owner approval; it
   has no scientific effect, and a passing cohort audit is required to finalize
   exclusions. The advisory `second_reviewer` sweep needs only a current
   semantic identity (section 3.1). V1 implements these advisory and routing admission changes.
2. "Prompt, schema, policy, workflow, parser, model, protocol, and runtime
   changes invalidate the appropriate semantic identity." Runtime (toolchain,
   target, deployment build) **no longer** invalidates. Identity scheme 2
   (appendix A) replaces the broad build hash with a narrow implementation
   boundary, golden behavioural fingerprints, a dependency-closure
   fingerprint, and the provider endpoint.
3. "Both origins can create proposals only." This still holds per record.
   AI-first finalization is a cohort action executed through ordinary
   screening events after owner approval, not a proposal accepted by an
   automation.

ADR 0004 decisions that are **superseded**: none.

## 3. Second-reviewer semantics

### 3.1 Advisory second reviewer (the existing `second_reviewer`)

- A human screens every record and holds all scientific authority.
- The AI gives an independent opinion, blinded until the human decides.
- Disagreements become conflicts that a person may resolve.

Because the human decides everything, the V1 advisory opinion can contribute
from record one. Remove the calibration-bundle prerequisite for the advisory
sweep and retain current-identity checks, the budget guard and blinding. This
changes the historic prerequisite without granting scientific authority to
the opinion. Calibration storage and stage-scoped admission remain available
for other consequential definitions and future validated replacement evidence.

Advisory mode still carries three risks:

- **Automation bias.** This is mitigated by blinding until decision.
- **Conflict workload.** The AI–human disagreement rate is monitored and shown
  to the owner. If it is too high, the owner should drop to `suggest` or
  `off`.
- **Contaminated independence.** Exposure provenance (Phase 1) keeps exposed
  pairs out of every metric that claims independence.

### 3.2 Human-replacement second reviewer (`dual_replacement`)

Replacing a human second screener is a scientific claim about **joint errors**.
For a relevant record, loss with an OR inclusion rule is
`P(human excludes AND AI excludes | relevant)`. A high marginal AI sensitivity
can coexist with complete overlap in human and AI errors. Unexposed decisions
protect against verdict exposure; they do not establish statistical
independence of errors.

**Recommendation: keep replacement unavailable until a separate, accepted
non-inferiority design measures the joint miss rate against dual human
screening on a random project-specific sample.** It needs a declared margin,
a sufficiently strong blinded reference, and sample planning for the rare
records the first human misses. This ADR does not select a margin or invent a
sample size for that comparison. Stage-scoped, current-identity bundles would
persist its evidence; advisory opinions can accrue while it is designed.

As a diagnostic only, one may report marginal AI sensitivity with a one-sided
95% Clopper–Pearson lower bound. A lower bound of 0.90 requires at least 29
reference-relevant records with zero AI misses (`0.05**(1/29) ≈ 0.902`). That
arithmetic does **not** validate human replacement. AI sensitivity conditional
on the human excluding a reference-relevant record is a necessary diagnostic,
not merely an optional display. Precision will often be poor because such
records are rare.

The 0.90 human sensitivity used in the workload scenarios is an assumption,
not a universal parity target. [Gartlehner et al. (2020)](https://www.rti.org/publication/single-reviewer-abstract-screening-missed-13-percent-relevant-studies-crowd-based-randomized-control)
reported 86.6% for single screening and 97.5% for dual screening on two review
topics. [Waffenschmidt et al. (2019)](https://doi.org/10.1186/s12874-019-0782-0)
found substantial variation across reviewers and reviews. Neither establishes
DeepRef project performance or the human–AI conditional error relationship.

### 3.3 Exposure policy

Phase 1 records exposure per **reviewer × record × stage**. A pair is exposed
if any AI screening verdict for that record and stage was _available_ to the
reviewer before their decision. Proof of viewing is not required. The rules:

| Surface                                                                              | Policy in second-reviewer mode                                                                         |
| ------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------ |
| Screening read path, conflicts list, activity feed, proposal reads for waiting pairs | **Blind** (implemented). The AI verdict is withheld until the human decides.                           |
| Resolve endpoint                                                                     | Refuses until the human has decided. The reveal is recorded as `reviewer_opinion_reveal`.              |
| Suggest-mode suggestions                                                             | Allowed; recorded as `screening_suggestion` when served for an undecided record.                       |
| Workflow routing ports, run output, literature alerts, email/Slack/HTTP integrations | Allowed (product features); recorded as `workflow_run_output` when the verdict is attached to records. |

Exposed pairs remain valid product data. They are excluded from independent
agreement, `dual_replacement` evidence and drift monitoring. Pairs with
unknown decision provenance count as exposed: they are unverifiable.

## 4. AI-first disposition model

Phase 1 introduced no dispositions; the separate layer below is selected for
V1 implementation. Temporary AI routing is **not** a
`screening_state` value. There is no `ai_quarantined` status. Instead:

```text
ai_screening_dispositions(
  id, project_id, report_id, stage,
  kind               -- 'quarantine_exclude' (V1 only kind)
  cohort_id,
  ai_run_id, review_run_id,
  evaluated_revision -- screening_state.revision the AI saw
  protocol_version_id, semantic_bundle_hash,
  policy_version     -- automation-eligible exclusion rule version
  created_at, voided_at, void_reason,
  finalized_at, finalized_event_id
)
```

- A quarantined record is scientifically **unscreened**. The disposition only
  removes it from the normal human queue. Queue filters and counters read it.
  `screening_state` never does.
- Recovery voids the disposition. The record reappears in the queue. Nothing
  scientific is undone, because nothing scientific happened.
- Any human screening event for the record and stage voids its disposition
  automatically. A human decision always supersedes routing.
- Dispositions are immutable apart from `voided_at`, `void_reason`,
  `finalized_at` and `finalized_event_id`, and only one of
  `voided_at`/`finalized_at` can ever be set.

## 5. Automation-eligible exclusion rule (`automation_eligible_exclusion` v1)

Self-reported LLM confidence is not used. DeepRef's screening schema has no
confidence field, and "high-confidence exclude" is banned in UI and
documentation unless it is empirically calibrated and defined. A record is
routed to quarantine only when **all** of these hold:

1. The run is a completed compiled screening run for the stage. Every node
   validated, and there was no repair-budget exhaustion and no
   human-adjudication block.
2. `suggested_decision.kind == exclude`. Title/abstract has no reason.
3. Primary and independent screens agree on `exclude`. The compiled workflow
   already runs an independent screen on every exclude, and its reconcile
   node blocks on disagreement.
4. **Mechanical implication:** at least one decisive criterion supports
   exclusion. An inclusion criterion is `does_not_meet`, or an exclusion
   criterion is `meets`. This is recomputed by code from the judgments, not
   taken from the model's decision.
5. **Grounding:** every decisive judgment has at least one evidence item that
   passed grounding validation for the stage. At title/abstract, the evidence
   must be a hashed title or abstract field of this report.
6. **No unresolved uncertainty on the decisive path:** `uncertainties` is
   empty, and no decisive criterion is `unclear`. `unclear` on a
   non-decisive criterion is allowed.
7. The record has a title, and an abstract of at least a minimum length
   (proposed: 50 characters). Records without an abstract go to people.
8. The current stage, ceiling and evidence authority permit `routing`, and the
   cohort is open under the same identity.

Everything else goes to people: AI includes, `maybe`, `insufficient_evidence`,
and excludes that fail any condition above. The rule is code, versioned
(`policy_version = 1`), and part of semantic identity through the golden-parse
fingerprint, which already exercises decision interpretation. When it is
implemented, its version must also be a declared identity component.

## 6. Audit streams and strata

Strata in a title/abstract cohort:

| Stratum                                            | Routing                                                    | Contribution to inference       |
| -------------------------------------------------- | ---------------------------------------------------------- | ------------------------------- |
| I: AI include                                      | normal human screening (no auto-include in V1, section 12) | R_IU (relevant found by people) |
| U: AI maybe / insufficient evidence / not eligible | complete human screening                                   | R_IU                            |
| X: automation-eligible exclusion                   | quarantine                                                 | probability sample → k, N_x     |

There are two streams, and they never mix:

- **Random audit (inferential).** A simple random sample without replacement
  from X, drawn using an unbiased algorithm with a stored seed after the frame
  and planner inputs are frozen. Only this stream
  feeds the estimator.
- **Targeted review (error discovery).** AI–AI disagreements, near-boundary
  outputs, repair cases, reviewer flags and downstream surprises. These are
  biased by construction. They are used for model and workflow learning and as
  drift triggers, never as a probability sample. Targeted moves to the human
  queue and their labels must be completed **before** the sampling frame, R_IU
  and n are frozen and before the random draw. Removed records are then outside
  X, and reference-relevant findings join R_IU. After the draw, frame membership
  and denominators cannot shrink selectively: a targeted record in the sample
  retains its sample label; a later targeted rescue outside the sample is
  recorded separately. Discoveries may invalidate the cohort and require full
  human screening, but do not retrospectively redefine its probability sample.

Audit records are **interleaved blind** into the normal queue. Reviewers do
not know a record came from X, and the AI opinion is withheld. Every audit
label carries exposure provenance. An exposed label is dropped (section 2.4).

## 7. Statistical estimator and cohort-finalization rule

### 7.1 Target

**Conditional automation recall:** among the fixed relevant records already
found in I ∪ U and the relevant records in the frozen X frame, the share retained
after the audit rescues its relevant findings. Relevance is defined by the
stage reference rule, and R_IU is treated as fixed and correctly classified.
This conditions on what the human strata found; relevant records people
missed in I ∪ U are absent from the denominator. It is consequently **not** a
bound on total true stage recall or on final included-study recall.

Human-stratum errors, reference-standard false negatives (especially on
AI-hard records), false positives in R_IU, and downstream errors need separate
assessment. The nominal finite-population guarantee assumes correct labels;
with an imperfect reference it is a bound relative to that rule, and must not
be advertised as a 95% guarantee of true scientific recall.

### 7.2 Closed cohort

A cohort is the set of records AI-evaluated at one stage under one pinned
semantic identity and one protocol version. It closes when one of three
things happens:

- the evaluation queue for the stage is exhausted;
- the budget stops;
- the owner closes it.

No record joins a closed cohort. Finish human screening in I ∪ U and any
pre-draw targeted moves, then freeze X, R_IU, n and the sampling algorithm before
drawing or inspecting audit labels. New records form a new cohort. V1 uses
**one look per cohort**; changing n or re-drawing after seeing k is prohibited.
A cohort may be abandoned early for failure, but cannot pass early. The review-wide nominal error budget is 0.05 under correct fixed reference
labels. Assign cohort j its one-sided level `α_j = 0.05 / 2^j`, starting at
j = 1, before drawing. The first level is 0.025, then 0.0125, and so on;
the infinite sum is 0.05. The union bound controls the chance of any false
conditional-reference certification across cohorts by 0.05 when each test is
valid conditional on its preceding history. This does not require independent
cohort errors, and does not control true recall with noisy labels.

The ordinal is durable and project-wide: never reset it on protocol changes,
identity changes, cohort cancellation, failed audits, retries or stage changes.
Registration reserves the level before the seed, sample or audit labels are
inspected; discarded audits do not reclaim it. A failed or exposed cohort may
return to complete human screening; it cannot be re-drawn until it happens to
pass. A later cohort represents new membership and a new registered ordinal.
V1 stores `floor(50,000,000 / 2^j)` billionths, a conservative downward
approximation to α_j, and permits ordinals 1 through 25. The next ordinal
refuses further automated audits and directs records to humans; it does not
reset the budget. The stored integer level is the comparison source of truth.
An extended policy must preserve spending rather than substitute a default
after numeric underflow.

### 7.3 Test

Notation:

- N_x: size of X at close (after removing targeted moves);
- n: sample size, fixed at close from the planning rule (7.4);
- k: sampled records labelled relevant by the reference standard (section 8).
  These are rescued for human screening/retention; include or maybe under the
  OR reference rule counts positive for inference;
- R_IU: the fixed count of correctly reference-relevant records already found
  in I ∪ U, including completed pre-draw targeted moves. In operation these
  are reference-labelled includes, so label error remains a limitation.

If X holds D reference-relevant records, the conditional automation recall is

```text
recall(D) = (R_IU + k) / (R_IU + D)
```

With correct fixed labels, the cohort passes when a one-sided confidence
bound excludes conditional automation recall < τ at level α. Let
D0 = floor((R_IU + k)/τ − R_IU) + 1 be the smallest D with recall < τ. Then

```text
p = P_hypergeometric(K ≤ k ; population N_x, successes D0, draws n)
pass  ⇔  p ≤ α
```

The hypergeometric CDF falls as D grows, so rejecting at D0 rejects every
D ≥ D0. Equivalently invert the hypergeometric CDF to obtain a one-sided
upper confidence bound on D and substitute it in the recall formula. For a
fixed true D, a false certification implies `P_D(K ≤ k) ≤ α`, whose probability
is at most α; this establishes finite-population coverage despite D0 depending
on k. V1 computes authorization by exact `BigUint` comparison of the
hypergeometric probability numerator times 10^9 with the denominator times
the stored alpha billionths. A log-space floating-point CDF is diagnostic
and may reject before the exact comparison, but cannot authorize by itself.
The reported `reference_retention` is the plug-in point estimate
`(R_IU+k)/(R_IU+k*N_x/n)`, not the confidence lower bound and not measured
true recall; it must be labelled an estimate. Handle D0 > N_x as an empty null (pass), a full census as no remaining
automation exclusions, and R_IU = D = 0 as no observed recall denominator
(report undefined recall rather than 100%).

This adapts the finite-population idea in
[Callaghan & Müller-Hansen (2020)](https://doi.org/10.1186/s13643-020-01521-4);
the conditional target and cohort policy here are DeepRef proposals, not a
claim validated by that paper. **V1 parameters:** τ is owner-selected from
0.95 and 0.98, with the cohort-specific α_j registered under section 7.2.
Imperfect human labels do not satisfy the exact-label premise; section 8 and
Table 4 quantify illustrative consequences, not restore coverage. Calling the
operational quantity conditional **reference retention** avoids representing
those labels as ground truth.

### 7.4 Sample size (planning)

V1 uses a transparent conservative **zero-miss minimum**: compute the smallest
integer n for which the section 7.3 rule passes if k = 0, using frozen N_x,
R_IU, τ and registered α_j. This chooses a valid fixed audit size without
inventing an estimate of true AI misses. The owner may choose a larger n,
including a census, before drawing. Freeze and store the selected n; it cannot
be enlarged after seeing labels in an attempt to convert failure into pass.

This is the minimum under a zero-finding scenario, **not** an 80% power plan.
If findings occur, evaluate the same rule at that k; the cohort may fail.
With D0(0) = floor(R_IU/τ − R_IU) + 1, the large-population approximation is
`n/N_x ≈ 1 − α_j^(1/D0(0))`. For R_IU = 100 and τ = 0.95, D0 = 6 and the first
cohort's α_1 = 0.025 implies roughly 45.9% of X, higher than the 39.3% from the
historical standalone α = 0.05 tables. Later ordinals increase that share.
Sample planning must display the registered α, finite-population n, selected
larger n if any, and the human-work forecast before approval.

A census leaves no automation exclusions and is accounted as human-reviewed
work. If R_IU = k = 0, recall has no observed denominator: report it undefined,
never 100%. V1 may require a positive reference-retained count before ordinary
statistical finalization; a complete census can end human screening without
pretending that an empty denominator establishes recall.

Section 9 retains the earlier ≥80% scenario-power calculations as supporting
economic illustrations at standalone α = 0.05. They are not operational V1
sample sizes, pass guarantees or permission gates. A future power-based
planner would need independent miss-rate evidence and recomputed tables for
the spending policy before it could replace this planner.

### 7.5 Outcome

- **Pass + owner approval:** each unsampled X record not voided gets an
  ordinary title/abstract `exclude` screening event. It is written by
  automation actor `ai-first-cohort:<cohort_id>`, with `expected_revision` =
  the disposition's evaluated revision, and with the cohort, approval,
  identity and policy ids in its notes or payload. Sampled records keep their
  human labels as their screening decisions.
- **Fail:** all dispositions are voided and the records return to the human
  queue. Routing is suspended pending owner re-approval.
- **Not finalizable / owner declines:** the same as fail, with no suspension.

### 7.6 Why not reuse the old calibration thresholds

The old 60 includes / 300 records / 95% point / 90% lower bound gate measured
an advisory opinion's sensitivity on human-decided records. AI-first risk is
the number of relevant records removed without a human look, and that depends
on N_x, R_IU and the audit. A fixed include count can neither guarantee nor
exclude a recall loss. Section 7.3 replaces it for workflow B. Section 3.2
requires a separate joint-error study before `dual_replacement`.

## 8. Audit reference standard

A single human screener is not a gold standard. Records the AI wrongly
excludes are disproportionately hard records, which human reviewers also miss
more often. That shared error makes k too small, and the estimated recall too
high. The options compared, with quantities from section 9, table 4:

| Option                                                  | Detection of a truly relevant audit record (illustrative)                                             | Judgments per audit record                      | Operational complexity | Protection against shared AI/human mistakes                                                                 |
| ------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- | ----------------------------------------------- | ---------------------- | ----------------------------------------------------------------------------------------------------------- |
| Single blinded expert                                   | ≈ 0.90 average; lower on AI-hard records                                                              | 1                                               | low                    | weak: one correlated miss is invisible                                                                      |
| Dual independent, OR rule (relevant if either includes) | ≈ 0.99 independent, ≈ 0.97 with correlated errors                                                     | 2                                               | medium                 | good: both must miss                                                                                        |
| Dual independent + adjudication                         | Unknown; an adjudicator may overturn a correct include                                                | ≈ 2.1 under the assumed 10% disagreement        | high                   | May improve precision; sensitivity requires measurement                                                     |
| Eventual downstream inclusion                           | not available for X: unsampled and excluded records never go downstream                               | 0 extra                                         | low                    | none for X; useful only to confirm k-records and for I ∪ U                                                  |
| **Hybrid (recommended)**                                | Dual-OR sensitivity remains unknown; 0.97 and 0.995 are scenario assumptions, not measured guarantees | 2 (+ normal full-text work for rescued records) | medium                 | Downstream confirmation can study precision; it cannot recover relevant records both audit reviewers missed |

**Recommendation: hybrid.**

- Two distinct human reviewer identities label each audit record, blinded to
  the AI verdict and to the other reference judgment until both have recorded
  their own. Exposure provenance applies to both. Self-asserted actor strings
  do not prove two different people; the owner attests to that limitation.
  Blinding prevents verdict copying but does not imply independent errors.
- For the estimator, a record is reference-positive if **either** labels it
  include or maybe. The OR rule defines the reference-positive population;
  retained maybe records need further human review. False reference positives
  and false negatives mean the bound is not true-eligibility recall. No
  adjudication is required for this declared reference rule.
- Audit reference-positive records continue to full text, including those
  retained through a raw maybe label. Maybe in the ordinary human strata
  keeps its normal workflow. Subsequent outcomes support precision analysis
  and model learning but do not retroactively reduce k or R_IU in the frozen
  cohort.
- Adjudication is optional and does not affect the estimator.
- One reviewer may be the person doing the stage's normal screening. Exposure
  rules apply to both.

**Cost:** two judgments per audit record. Section 9 counts this cost in full.
Where it erases the savings, section 10 tells the user.

**Residual bias:** all numerical reference sensitivities are assumptions. If
the first reviewer misses 10% and the second misses 30% of those, the joint
miss rate is 3%; neither value is measured for DeepRef or specifically for
AI-excluded records. Table 4 uses independent per-record binomial detection
with fixed sensitivity and no false positives. Its small biases and false-pass
values apply only to those scenarios; shared difficulty across records or
lower sensitivity can produce greater errors and invalidate nominal α. The
0.995 full-text-check row is an optimistic sensitivity scenario, not an
operational claim that downstream checks recover records rejected by both
auditors. V1 cannot reduce k after full-text review. Reference quality and
acceptance of this residual risk remain owner decisions.

A concrete counterexample shows why Table 4 is not a uniform safety bound:
with R_IU = 10, N_x = 1,000, D = 1 and n = 950, the perfect-label rule passes
when k = 0 with probability 0.05. If each sampled relevant record is detected
with sensitivity 0.90, an unrescued miss is hidden with probability
`0.05 + 0.95 × 0.10 = 0.145`; the rule still passes on those observations.
True conditional recall is then `10/11 ≈ 0.909`, below 0.95. Even sensitivity
0.97 gives false certification 0.0785. These are deductions under the stated
binomial-detection model. A claim of true-recall control would require a
validated reference-error correction or stronger evidence; dual labels alone
do not supply it.

## 9. Workload and break-even

The tables below are historical **standalone α = 0.05, 80%-scenario-power**
illustrations from the earlier draft. V1 instead spends α_j and uses the
zero-miss minimum in section 7.4; these tables must not be shown as the actual
current audit forecast or used to claim its power. They remain reproducible
supporting evidence for how audit burden changes with yield and performance.

Unit: one **human record judgment**. One adjudication counts as one judgment.
AI processing is reported separately as AI cost. The model, parameters and
exact computations are in the scenario notes below and are reproducible with
the stored script. Every AI, human, disagreement and prevalence parameter is
a scenario assumption; these are forecasts, not measured DeepRef performance.

The power tables optimistically set R_IU to all relevant records retained by
the AI before human errors (`Rel × r_ai`); the loss tables then separately
apply human misses. A production plan must use the actually frozen count
found by people, so these optimistic audit sizes are not a jointly calibrated
forecast of total recall. Rounding expected misses to zero can make power
0 or 1 for small cohorts; a stochastic prevalence model would give different
values. The workload formulas assume the proposed audit successfully
finalizes X. They exclude re-screening on audit failure or declined approval,
additional targeted reviews, administration, full-text retrieval and AI spend.
A fallback forecast must add human screening of the remaining X. The loss
columns are unconditional scenario losses under finalization, not losses
conditional on passing and not the expected losses of the fail-and-fallback
policy. They optimistically assume perfect rescue in the audit; Table 4
explores missed audit labels separately.

#### Parameters

| Parameter                             | Value                                                                                                                                                                                              |
| ------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Target recall tau                     | 0.95 (Tables 1-4); 0.90 and 0.98 in Table 1b                                                                                                                                                       |
| One-sided alpha                       | 0.05                                                                                                                                                                                               |
| Power target                          | 0.80                                                                                                                                                                                               |
| Human single-screener sensitivity s_h | 0.90 (expected-loss column only)                                                                                                                                                                   |
| Human-human disagreement c_hh         | 0.10 (one adjudication per disagreement)                                                                                                                                                           |
| AI-human disagreement c_ha            | 0.12 (strong), 0.20 (weak)                                                                                                                                                                         |
| Reference-standard disagreement c_ref | 0.10                                                                                                                                                                                               |
| AI profile strong                     | r_ai = 0.99, q_x = 0.85                                                                                                                                                                            |
| AI profile weak                       | r_ai = 0.965, q_x = 0.60                                                                                                                                                                           |
| N (records)                           | 2,000; 5,000; 20,000; 50,000                                                                                                                                                                       |
| Prevalence pi                         | 0.5%; 2.0%; 10.0%                                                                                                                                                                                  |
| Derived counts                        | Rel = pi N; Irr = (1-pi) N; D = Rel (1-r_ai); N_x = D + q_x Irr; N_h = N - N_x; R_IU = Rel r_ai (exact expectations, Fraction arithmetic)                                                          |
| Counts used in formulas               | N_x, N_h, R_IU, D_true = round(D) to nearest integer (ties up)                                                                                                                                     |
| Recommendation rule                   | AI-first recommended if achievable and W_af <= 0.8 x W_base and W_base - W_af >= 500; AI-first allowed, little benefit if achievable otherwise; AI-first not achievable if no n reaches power 0.80 |

Method notes: n* is the exact minimal n in [0, N_x] from a step-1 scan starting at n = 0 (power is not monotone, so no binary search). The SPEC coarse grid + refinement is also run and is reported in Table 2 diagnostics. Ties (p exactly equal to alpha, possible when D0 = 1 and N_x is a multiple of 20) count as a pass and are decided with integer arithmetic. Expected losses use exact expectations. Table 4's realised recall for an observed count k_obs is (R_IU + k_obs)/(R_IU + D_true), because a relevant record the reference standard misses is not rescued.

#### Table 1: audit fraction needed when the audit finds zero relevant (tau = 0.95, alpha = 0.05)

| R_IU  | D0(0) | Large-N f = 1-alpha^(1/D0) | N_x = 1,000 | N_x = 5,000   | N_x = 20,000   | N_x = 50,000   |
| ----- | ----- | -------------------------- | ----------- | ------------- | -------------- | -------------- |
| 10    | 1     | 95.0%                      | 950 (95.0%) | 4,750 (95.0%) | 19,000 (95.0%) | 47,500 (95.0%) |
| 20    | 2     | 77.6%                      | 777 (77.7%) | 3,882 (77.6%) | 15,528 (77.6%) | 38,820 (77.6%) |
| 50    | 3     | 63.2%                      | 631 (63.1%) | 3,158 (63.2%) | 12,632 (63.2%) | 31,580 (63.2%) |
| 100   | 6     | 39.3%                      | 393 (39.3%) | 1,965 (39.3%) | 7,860 (39.3%)  | 19,651 (39.3%) |
| 200   | 11    | 23.8%                      | 238 (23.8%) | 1,191 (23.8%) | 4,767 (23.8%)  | 11,920 (23.8%) |
| 500   | 27    | 10.5%                      | 104 (10.4%) | 524 (10.5%)   | 2,100 (10.5%)  | 5,250 (10.5%)  |
| 1,000 | 53    | 5.5%                       | 54 (5.4%)   | 274 (5.5%)    | 1,098 (5.5%)   | 2,747 (5.5%)   |
| 2,000 | 106   | 2.8%                       | 27 (2.7%)   | 138 (2.8%)    | 556 (2.8%)     | 1,392 (2.8%)   |

#### Table 1b: same at tau = 0.90 and tau = 0.98 (N_x = 20,000)

| R_IU  | tau=0.90: D0(0) | f approx | tau=0.90: n0 (share) | tau=0.98: D0(0) | f approx | tau=0.98: n0 (share) |
| ----- | --------------- | -------- | -------------------- | --------------- | -------- | -------------------- |
| 10    | 2               | 77.6%    | 15,528 (77.6%)       | 1               | 95.0%    | 19,000 (95.0%)       |
| 20    | 3               | 63.2%    | 12,632 (63.2%)       | 1               | 95.0%    | 19,000 (95.0%)       |
| 50    | 6               | 39.3%    | 7,860 (39.3%)        | 2               | 77.6%    | 15,528 (77.6%)       |
| 100   | 12              | 22.1%    | 4,418 (22.1%)        | 3               | 63.2%    | 12,632 (63.2%)       |
| 200   | 23              | 12.2%    | 2,442 (12.2%)        | 5               | 45.1%    | 9,014 (45.1%)        |
| 500   | 56              | 5.2%     | 1,041 (5.2%)         | 11              | 23.8%    | 4,767 (23.8%)        |
| 1,000 | 112             | 2.6%     | 527 (2.6%)           | 21              | 13.3%    | 2,658 (13.3%)        |
| 2,000 | 223             | 1.3%     | 266 (1.3%)           | 41              | 7.0%     | 1,408 (7.0%)         |

#### Table 2a: planned sample size with power (SPEC scenario grid)

| N      | pi    | AI     | Rel   | N_x    | N_h    | R_IU  | D (expected) | D_true | n* (power >= 0.80) | n*/N_x | power(n*) |
| ------ | ----- | ------ | ----- | ------ | ------ | ----- | ------------ | ------ | ------------------ | ------ | --------- |
| 2,000  | 0.5%  | strong | 10    | 1,692  | 308    | 10    | 0.10         | 0      | 1,608              | 95.0%  | 100.0%    |
| 2,000  | 0.5%  | weak   | 10    | 1,194  | 806    | 10    | 0.35         | 0      | 1,135              | 95.1%  | 100.0%    |
| 2,000  | 2.0%  | strong | 40    | 1,666  | 334    | 40    | 0.40         | 0      | 1,052              | 63.1%  | 100.0%    |
| 2,000  | 2.0%  | weak   | 40    | 1,177  | 823    | 39    | 1.40         | 1      | 884                | 75.1%  | 100.0%    |
| 2,000  | 10.0% | strong | 200   | 1,532  | 468    | 198   | 2.00         | 2      | 518                | 33.8%  | 88.6%     |
| 2,000  | 10.0% | weak   | 200   | 1,087  | 913    | 193   | 7.00         | 7      | 595                | 54.7%  | 90.1%     |
| 5,000  | 0.5%  | strong | 25    | 4,229  | 771    | 25    | 0.25         | 0      | 3,283              | 77.6%  | 100.0%    |
| 5,000  | 0.5%  | weak   | 25    | 2,986  | 2,014  | 24    | 0.88         | 1      | 2,582              | 86.5%  | 100.0%    |
| 5,000  | 2.0%  | strong | 100   | 4,166  | 834    | 99    | 1.00         | 1      | 2,169              | 52.1%  | 100.0%    |
| 5,000  | 2.0%  | weak   | 100   | 2,944  | 2,056  | 97    | 3.50         | 4      | 1,928              | 65.5%  | 81.6%     |
| 5,000  | 10.0% | strong | 500   | 3,830  | 1,170  | 495   | 5.00         | 5      | 606                | 15.8%  | 82.0%     |
| 5,000  | 10.0% | weak   | 500   | 2,718  | 2,282  | 483   | 17.50        | 18     | 1,101              | 40.5%  | 85.6%     |
| 20,000 | 0.5%  | strong | 100   | 16,916 | 3,084  | 99    | 1.00         | 1      | 8,808              | 52.1%  | 100.0%    |
| 20,000 | 0.5%  | weak   | 100   | 11,944 | 8,056  | 97    | 3.50         | 4      | 7,823              | 65.5%  | 81.6%     |
| 20,000 | 2.0%  | strong | 400   | 16,664 | 3,336  | 396   | 4.00         | 4      | 3,300              | 19.8%  | 82.2%     |
| 20,000 | 2.0%  | weak   | 400   | 11,774 | 8,226  | 386   | 14.00        | 14     | 4,928              | 41.9%  | 81.4%     |
| 20,000 | 10.0% | strong | 2,000 | 15,320 | 4,680  | 1,980 | 20.00        | 20     | 882                | 5.8%   | 89.5%     |
| 20,000 | 10.0% | weak   | 2,000 | 10,870 | 9,130  | 1,930 | 70.00        | 70     | 2,224              | 20.5%  | 82.9%     |
| 50,000 | 0.5%  | strong | 250   | 42,290 | 7,710  | 248   | 2.50         | 3      | 11,815             | 27.9%  | 80.9%     |
| 50,000 | 0.5%  | weak   | 250   | 29,859 | 20,141 | 241   | 8.75         | 9      | 15,162             | 50.8%  | 90.2%     |
| 50,000 | 2.0%  | strong | 1,000 | 41,660 | 8,340  | 990   | 10.00        | 10     | 4,584              | 11.0%  | 91.2%     |
| 50,000 | 2.0%  | weak   | 1,000 | 29,435 | 20,565 | 965   | 35.00        | 35     | 8,416              | 28.6%  | 82.6%     |
| 50,000 | 10.0% | strong | 5,000 | 38,300 | 11,700 | 4,950 | 50.00        | 50     | 908                | 2.4%   | 88.5%     |
| 50,000 | 10.0% | weak   | 5,000 | 27,175 | 22,825 | 4,825 | 175.00       | 175    | 3,214              | 11.8%  | 81.6%     |

#### Table 2b: workloads (human record judgments) and savings

| N      | pi    | AI     | W_single | W_dual  | W_adv_r | W_af_2 | W_af_2d | Savings W_af_2 vs W_single | Savings W_af_2d vs W_dual |
| ------ | ----- | ------ | -------- | ------- | ------- | ------ | ------- | -------------------------- | ------------------------- |
| 2,000  | 0.5%  | strong | 2,000    | 4,200   | 2,240   | 3,524  | 3,863   | -76.2%                     | 8.0%                      |
| 2,000  | 0.5%  | weak   | 2,000    | 4,200   | 2,400   | 3,076  | 3,963   | -53.8%                     | 5.7%                      |
| 2,000  | 2.0%  | strong | 2,000    | 4,200   | 2,240   | 2,438  | 2,805   | -21.9%                     | 33.2%                     |
| 2,000  | 2.0%  | weak   | 2,000    | 4,200   | 2,400   | 2,591  | 3,496   | -29.5%                     | 16.8%                     |
| 2,000  | 10.0% | strong | 2,000    | 4,200   | 2,240   | 1,504  | 2,019   | 24.8%                      | 51.9%                     |
| 2,000  | 10.0% | weak   | 2,000    | 4,200   | 2,400   | 2,103  | 3,107   | -5.1%                      | 26.0%                     |
| 5,000  | 0.5%  | strong | 5,000    | 10,500  | 5,600   | 7,337  | 8,185   | -46.7%                     | 22.0%                     |
| 5,000  | 0.5%  | weak   | 5,000    | 10,500  | 6,000   | 7,178  | 9,393   | -43.6%                     | 10.5%                     |
| 5,000  | 2.0%  | strong | 5,000    | 10,500  | 5,600   | 5,172  | 6,089   | -3.4%                      | 42.0%                     |
| 5,000  | 2.0%  | weak   | 5,000    | 10,500  | 6,000   | 5,912  | 8,174   | -18.2%                     | 22.2%                     |
| 5,000  | 10.0% | strong | 5,000    | 10,500  | 5,600   | 2,382  | 3,669   | 52.4%                      | 65.1%                     |
| 5,000  | 10.0% | weak   | 5,000    | 10,500  | 6,000   | 4,484  | 6,994   | 10.3%                      | 33.4%                     |
| 20,000 | 0.5%  | strong | 20,000   | 42,000  | 22,400  | 20,700 | 24,092  | -3.5%                      | 42.6%                     |
| 20,000 | 0.5%  | weak   | 20,000   | 42,000  | 24,000  | 23,702 | 32,564  | -18.5%                     | 22.5%                     |
| 20,000 | 2.0%  | strong | 20,000   | 42,000  | 22,400  | 9,936  | 13,606  | 50.3%                      | 67.6%                     |
| 20,000 | 2.0%  | weak   | 20,000   | 42,000  | 24,000  | 18,082 | 27,131  | 9.6%                       | 35.4%                     |
| 20,000 | 10.0% | strong | 20,000   | 42,000  | 22,400  | 6,444  | 11,592  | 67.8%                      | 72.4%                     |
| 20,000 | 10.0% | weak   | 20,000   | 42,000  | 24,000  | 13,578 | 23,621  | 32.1%                      | 43.8%                     |
| 50,000 | 0.5%  | strong | 50,000   | 105,000 | 56,000  | 31,340 | 39,821  | 37.3%                      | 62.1%                     |
| 50,000 | 0.5%  | weak   | 50,000   | 105,000 | 60,000  | 50,465 | 72,620  | -0.9%                      | 30.8%                     |
| 50,000 | 2.0%  | strong | 50,000   | 105,000 | 56,000  | 17,508 | 26,682  | 65.0%                      | 74.6%                     |
| 50,000 | 2.0%  | weak   | 50,000   | 105,000 | 60,000  | 37,397 | 60,019  | 25.2%                      | 42.8%                     |
| 50,000 | 10.0% | strong | 50,000   | 105,000 | 56,000  | 13,516 | 26,386  | 73.0%                      | 74.9%                     |
| 50,000 | 10.0% | weak   | 50,000   | 105,000 | 60,000  | 29,253 | 54,361  | 41.5%                      | 48.2%                     |

Workloads are shown rounded to the nearest integer; savings use unrounded values. Negative savings mean more work than the baseline.

#### Table 2c: expected relevant records lost at the stage, and recommendation

| N      | pi    | AI     | Lost: single | Lost: dual | Lost: AI-first (W_af_2) | Lost: AI-first, dual baseline (W_af_2d) | Recommendation vs single (W_af_2 vs W_single) | Recommendation vs dual (W_af_2d vs W_dual) |
| ------ | ----- | ------ | ------------ | ---------- | ----------------------- | --------------------------------------- | --------------------------------------------- | ------------------------------------------ |
| 2,000  | 0.5%  | strong | 1.0          | 0.3        | 1.0                     | 0.3                                     | AI-first allowed, little benefit              | AI-first allowed, little benefit           |
| 2,000  | 0.5%  | weak   | 1.0          | 0.3        | 1.0                     | 0.3                                     | AI-first allowed, little benefit              | AI-first allowed, little benefit           |
| 2,000  | 2.0%  | strong | 4.0          | 1.2        | 4.1                     | 1.3                                     | AI-first allowed, little benefit              | AI-first recommended                       |
| 2,000  | 2.0%  | weak   | 4.0          | 1.2        | 4.2                     | 1.5                                     | AI-first allowed, little benefit              | AI-first allowed, little benefit           |
| 2,000  | 10.0% | strong | 20.0         | 6.0        | 21.1                    | 7.3                                     | AI-first allowed, little benefit              | AI-first recommended                       |
| 2,000  | 10.0% | weak   | 20.0         | 6.0        | 22.5                    | 9.0                                     | AI-first allowed, little benefit              | AI-first recommended                       |
| 5,000  | 0.5%  | strong | 2.5          | 0.8        | 2.5                     | 0.8                                     | AI-first allowed, little benefit              | AI-first recommended                       |
| 5,000  | 0.5%  | weak   | 2.5          | 0.8        | 2.5                     | 0.8                                     | AI-first allowed, little benefit              | AI-first allowed, little benefit           |
| 5,000  | 2.0%  | strong | 10.0         | 3.0        | 10.4                    | 3.4                                     | AI-first allowed, little benefit              | AI-first recommended                       |
| 5,000  | 2.0%  | weak   | 10.0         | 3.0        | 10.9                    | 4.1                                     | AI-first allowed, little benefit              | AI-first recommended                       |
| 5,000  | 10.0% | strong | 50.0         | 15.0       | 53.7                    | 19.1                                    | AI-first recommended                          | AI-first recommended                       |
| 5,000  | 10.0% | weak   | 50.0         | 15.0       | 58.7                    | 24.9                                    | AI-first allowed, little benefit              | AI-first recommended                       |
| 20,000 | 0.5%  | strong | 10.0         | 3.0        | 10.4                    | 3.4                                     | AI-first allowed, little benefit              | AI-first recommended                       |
| 20,000 | 0.5%  | weak   | 10.0         | 3.0        | 10.9                    | 4.1                                     | AI-first allowed, little benefit              | AI-first recommended                       |
| 20,000 | 2.0%  | strong | 40.0         | 12.0       | 42.8                    | 15.1                                    | AI-first recommended                          | AI-first recommended                       |
| 20,000 | 2.0%  | weak   | 40.0         | 12.0       | 46.7                    | 19.7                                    | AI-first allowed, little benefit              | AI-first recommended                       |
| 20,000 | 10.0% | strong | 200.0        | 60.0       | 216.8                   | 78.2                                    | AI-first recommended                          | AI-first recommended                       |
| 20,000 | 10.0% | weak   | 200.0        | 60.0       | 248.7                   | 113.6                                   | AI-first recommended                          | AI-first recommended                       |
| 50,000 | 0.5%  | strong | 25.0         | 7.5        | 26.6                    | 9.2                                     | AI-first recommended                          | AI-first recommended                       |
| 50,000 | 0.5%  | weak   | 25.0         | 7.5        | 28.4                    | 11.5                                    | AI-first allowed, little benefit              | AI-first recommended                       |
| 50,000 | 2.0%  | strong | 100.0        | 30.0       | 107.9                   | 38.6                                    | AI-first recommended                          | AI-first recommended                       |
| 50,000 | 2.0%  | weak   | 100.0        | 30.0       | 121.5                   | 53.9                                    | AI-first recommended                          | AI-first recommended                       |
| 50,000 | 10.0% | strong | 500.0        | 150.0      | 543.8                   | 197.3                                   | AI-first recommended                          | AI-first recommended                       |
| 50,000 | 10.0% | weak   | 500.0        | 150.0      | 636.8                   | 299.1                                   | AI-first recommended                          | AI-first recommended                       |

#### Table 3: realism check, N = 20,000, pi = 2%, q_x = 0.85

| r_ai  | D (expected) | D_true | N_x    | R_IU | n*    | n*/N_x | power(n*) | W_af_2 | Savings vs W_single | Recommendation                   |
| ----- | ------------ | ------ | ------ | ---- | ----- | ------ | --------- | ------ | ------------------- | -------------------------------- |
| 0.950 | 20.00        | 20     | 16,680 | 380  | 8,698 | 52.1%  | 82.3%     | 20,716 | -3.6%               | AI-first allowed, little benefit |
| 0.960 | 16.00        | 16     | 16,676 | 384  | 7,754 | 46.5%  | 84.9%     | 18,832 | 5.8%                | AI-first allowed, little benefit |
| 0.970 | 12.00        | 12     | 16,672 | 388  | 6,535 | 39.2%  | 85.6%     | 16,398 | 18.0%               | AI-first allowed, little benefit |
| 0.980 | 8.00         | 8      | 16,668 | 392  | 4,870 | 29.2%  | 82.0%     | 13,072 | 34.6%               | AI-first recommended             |
| 0.990 | 4.00         | 4      | 16,664 | 396  | 3,300 | 19.8%  | 82.2%     | 9,936  | 50.3%               | AI-first recommended             |
| 0.995 | 2.00         | 2      | 16,662 | 398  | 3,168 | 19.0%  | 96.4%     | 9,674  | 51.6%               | AI-first recommended             |

#### Table 4: reference-standard bias (N = 20,000, pi = 2%, n = n* = 3,300 from Table 2, strong AI: N_x = 16,664, R_IU = 396, D_true = 4)

Realised recall = (R_IU + k_obs)/(R_IU + D_true). Point estimate = (R_IU + k_obs)/(R_IU + k_obs N_x/n). The false-pass column uses a bad AI with r_ai = 0.93 (true recall before audit 0.930; N_x = 16,688, D_true = 28, R_IU = 372) and the same n.

| Reference standard           | s_ref | E[k_obs] (strong) | E[realised recall] (strong) | E[point estimate] (strong) | Upward bias, percentage points (estimate - realised) | P(pass), strong AI | P(pass), bad AI (false pass) |
| ---------------------------- | ----- | ----------------- | --------------------------- | -------------------------- | ---------------------------------------------------- | ------------------ | ---------------------------- |
| perfect reference (baseline) | 1.000 | 0.79              | 99.2%                       | 99.2%                      | +0.01 pp                                             | 82.2%              | 0.2%                         |
| single reviewer              | 0.900 | 0.71              | 99.2%                       | 99.3%                      | +0.11 pp                                             | 85.2%              | 0.4%                         |
| dual, independent            | 0.990 | 0.78              | 99.2%                       | 99.2%                      | +0.02 pp                                             | 82.5%              | 0.2%                         |
| dual, correlated             | 0.970 | 0.77              | 99.2%                       | 99.2%                      | +0.04 pp                                             | 83.1%              | 0.3%                         |
| dual + full-text check       | 0.995 | 0.79              | 99.2%                       | 99.2%                      | +0.01 pp                                             | 82.4%              | 0.2%                         |

The bias is shown in percentage points with 2 decimals because the values are small.

#### Table 4b (extra diagnostic, not in SPEC): false pass at the recall boundary

Hypothetical AI with R_IU = 372 and D_true = 20: true recall before audit 94.9% (just below 0.95), N_x = 16,680, n = 3,300. Pass probability at the boundary D = D0(0) = 20, the smallest D whose recall is below 0.95:

| Reference standard           | s_ref | P(pass) at boundary (D = 20) |
| ---------------------------- | ----- | ---------------------------- |
| perfect reference (baseline) | 1.000 | 1.2%                         |
| single reviewer              | 0.900 | 2.0%                         |
| dual, independent            | 0.990 | 1.3%                         |
| dual, correlated             | 0.970 | 1.4%                         |
| dual + full-text check       | 0.995 | 1.2%                         |

### 9.1 Reading the tables

These findings follow directly from the tables above. Note that the tables
treat each title/abstract stage as one closed cohort; smaller cohorts make
everything worse.

1. **The audit share is driven by how many relevant records people have
   already found, not by the size of the quarantine.** With zero misses the
   audit must cover ≈ 1 − α^(1/D0) of X:
   - 95% when people found 10 relevant records;
   - 39% at 100;
   - 10.5% at 500;
   - 5.5% at 1,000;
   - roughly the same at every N_x (table 1).
     A stricter τ = 0.98 multiplies the burden by 2–5× (table 1b).
2. **Small or low-yield reviews should not use AI-first.** Against single
   screening it adds work in every 2,000-record scenario, and in nearly every
   0.5% and 2% prevalence scenario below 20,000 records (table 2b). Dual audit
   labels are the reason: one quarantined record sampled costs two judgments.
3. **Large reviews with many relevant records save a lot.** The 20,000-record
   and 50,000-record stages at 2–10% prevalence with a strong AI save 50–73%
   of single-screening judgments.
4. **Dual-screening projects benefit most.** Against dual screening, AI-first
   with dual-screened human strata (W_af_2d) saves work in every scenario:
   6–75%.
5. **AI quality matters through the miss count D, not only through the
   quarantine size.** At 20,000 records and 2% prevalence (table 3), savings
   are:
   - 50% at r_ai = 0.99;
   - 35% at 0.98;
   - 18% at 0.97;
   - negative at 0.95.
     An AI below the pre-audit target may pass after enough records are rescued;
     the test concerns post-audit conditional recall. Table 4 shows pass
     probabilities ≤ 0.4% for its r_ai = 0.93 scenario, and Table 4b ≤ 2% for
     its D = 20 scenario. Those are examples, not uniform error guarantees
     with an imperfect reference.
6. **Protection is traded, not free.** With τ = 0.95, AI-first loses more
   relevant records than the matching baseline: up to +27% versus single
   screening and up to 2× versus dual screening, in the weak-AI, high-prevalence
   scenarios (table 2c). The workload recommendation therefore always appears
   _next to_ the expected-loss forecast (section 10). A stricter τ narrows the
   gap at the cost of audit size.
7. **The reference standard barely moves the recall estimate but does move the
   pass decision.** A single reviewer inflates the point estimate by only
   0.11 percentage points. It does raise the false-pass probability: 0.2% →
   0.4% for a bad AI, and 1.2% → 2.0% at the recall boundary (tables 4/4b).
   Assumed higher audit sensitivity keeps both near the perfect-reference
   values in these examples only. It does not establish real reference quality.
8. **Power is not monotone in n.** It is a step function, because the critical
   k changes with n. Validity holds for any n fixed before labelling. For
   planning, the proposal and tables use the smallest n with scenario
   power ≥ 0.80. No 2% stability margin is computed or promised. Changing
   that policy requires recomputing the tables and workload forecasts.
9. **Advisory second reviewer** costs N to N + c_ha·N in this model:
   12–20% above single screening if all disagreements are adjudicated. The
   model assigns no reduction in per-record judgment time and assumes no
   improvement in human recall from conflict resolution. These are bounded
   workload comparisons, not a claim about every deployment.
10. **Full text.** These tables model title/abstract only. The zero-miss
    formula illustrates high burden when few relevant records have been
    found, but no full-text prevalence, cost or recall data are estimated
    here. Excluding full-text AI-first in V1 is a conservative scope choice,
    subject to a separate stage analysis.

The computation script is checked in at
[`0005/workload.py`](0005/workload.py) (standard library only). It reproduces
every table: `python3 -I docs/adr/0005/workload.py`. It writes
[`results.md`](0005/results.md) and [`results.json`](0005/results.json) beside
itself; both are committed supporting evidence. The results contain numerical
self-checks, and repeated runs reproduce all scenario values.

## 10. Product recommendation threshold

An economic recommendation cannot supply scientific permission. Until the
owner accepts the reference-standard limitations and remaining methodological
choices, these table classifications are illustrative planning categories.
Before the owner approves
`routing`, and again before the frozen cohort's draw, V1 must show a forecast
using its actual α_j and selected n, with:

- the expected total human judgments for AI-first, including two judgments per
  audit record;
- the comparable baseline: single screening if the project screens singly,
  dual if it screens dually;
- the expected relevant records lost, reference-quality assumptions,
  uncertainty ranges, and workload if the audit fails or approval is declined.

V1 can recommend AI-first on workload grounds only if the selected audit's
**successful-finalization** workload is ≤80% of the comparable baseline and
saves at least 500 judgments (6.25 hours at an assumed 45 s each). It must show
this as a conditional forecast, with failure/declined-approval cost alongside;
no ≥80% power assertion is made by the zero-miss planner. For single human
screening in I ∪ U and a dual-reference audit, `W_pass = N_h + 2(n + N_c)`. Returning
all remaining X to one human after failure gives `W_fallback = N + n + 2N_c`, assuming
audited records do not need duplicate judgments. Targeted and downstream
work remain additional costs. Dual-human baseline projects need a separate
forecast for dual screening of their human strata.

Otherwise it shows one of two messages:

- "AI-first is allowed, but unlikely to reduce human workload for this review.
  Use the advisory second reviewer instead," when it is achievable but below
  the threshold;
- "AI-first cannot be validated for this review with a feasible audit,"
  when the selected predeclared audit/workload cap is infeasible.

The forecast is refreshed from observed N_x and R_IU as the cohort fills;
its inferential frame and selected n are frozen before sampling. The owner may still opt in when
AI-first is allowed but not recommended. The decision and the forecast shown
are recorded with the approval.

## 11. Comparison of the three primary workflows

| Dimension             | A. Standard dual human screening                                                                       | B. Human + advisory AI second reviewer                                                                                    | C. AI-first + selective human review + statistical audit                                                                           |
| --------------------- | ------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| Scientific protection | Two independent human judgments; 3% loss in the stated correlated-error scenario, not a universal rate | Human decides every record; blind conflicts may reveal some misses                                                        | Conditional automation bound under correct reference labels; human-stratum and reference errors are outside that guarantee         |
| Human workload        | ≈ 2.1 N                                                                                                | ≈ N (+ conflict review)                                                                                                   | N_h + 2(n + N_c) (section 9). It can be well below N for large, rich stages, and above N for small or low-yield ones               |
| AI cost               | none                                                                                                   | One compiled run per record, potentially multiple model calls for independent screens, reconciliation and bounded repairs | Compiled evaluations plus human audit; budget depends on model routes, tokens, repairs and follow-up work                          |
| Time-to-value         | none                                                                                                   | from record one                                                                                                           | after the cohort closes and the audit is labelled                                                                                  |
| Audit complexity      | none                                                                                                   | low (blinding, exposure provenance)                                                                                       | high (cohorts, seeded sampling, dual blinded labels, approvals)                                                                    |
| Reproducibility       | human events                                                                                           | human events + immutable AI runs                                                                                          | full recomputation from stored evidence (section 17)                                                                               |
| Automation-bias risk  | No AI verdict exposure in this workflow                                                                | Reduced by blinding; intentional exposure is tracked                                                                      | Humans screen an AI-filtered queue; blinded audit interleaving mitigates exposure, with residual selection effects                 |
| Missed-study risk     | Human errors remain and depend on protocol and reviewers                                               | Human errors remain; conflict resolution may help                                                                         | Only conditional automation loss is statistically assessed under the reference assumptions; poor AI or low yield can erase savings |

## 12. Auto-include: scientific vs economic risk

A false title/abstract include does not reduce recall. Its cost is economic:

- full-text retrieval;
- parsing;
- embeddings;
- full-text AI runs;
- the project AI budget;
- human full-text time.

The authority model keeps the two risks apart:

- Scientific authority concerns **exclusion** behaviour only, and is gated by
  section 7.
- Auto-include is not in V1. AI includes go to human title/abstract screening.
  A future `auto_include` would be an owner-approved **efficiency** setting.
  It would be gated by a precision forecast (expected full-text records and AI
  spend versus the budget), not by the recall test. It must never be presented
  as scientific validation.

## 13. Protocol amendments

On protocol republish (V1, conservative):

| Item                            | Behaviour                                                                                                                                                                     |
| ------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Open dispositions               | voided; records return to the human queue                                                                                                                                     |
| Open audit samples              | discarded for inference; labels are kept as learning data                                                                                                                     |
| Closed, unfinalized cohorts     | invalidated, never finalized                                                                                                                                                  |
| Authority state                 | evidence authority reset to identity-only states; the owner ceiling is unchanged; `routing` restarts only with a new cohort under the new identity and a fresh owner approval |
| Calibration evidence            | stale through the `Protocol` identity component (Phase 1 typed reason)                                                                                                        |
| Pending AI work                 | in-flight runs fail the existing subject-currency check at finalization; queued AI-first evaluations under the old identity are cancelled                                     |
| Finalized automation exclusions | any protocol republish invalidates the cohort and reopens its still-canonical automation exclusions through ordinary undo events; newer human decisions remain canonical      |

The protocol hash is shared across stages and is conservative: any republish
invalidates both stages. That is accepted for V1. A future identity scheme may
hash per-stage criteria. It would do so only with the golden fixtures proving
that the other stage's rendered requests are unchanged.

## 14. AI budget exhaustion

- AI-first ranks the frozen intake by SHA-256 of the domain string
  `ai-first-routing-v1`, the random cohort UUID and report UUID. The seed is
  fixed when the intake is created; later imports require a later cohort.
  This reproducible pseudorandom scheduling order is distinct from the exact
  Fisher-Yates audit draw and does not establish a probability sample of
  completed AI runs: failures and human rescues may affect completion.
- On exhaustion, no new dispositions are created. The cohort closes with
  partial AI coverage. Unprocessed intake remains in the human reference
  stratum; only actual quarantined records enter X for the fixed audit.
- Uncovered records were never AI-evaluated. They stay in the normal human
  queue as ordinary unscreened records. Nothing disappears.
- The UI reports completed compiled AI runs / frozen intake size and the
  cohort's own counts. Terminal exports preserve per-member run links.
- Audit labelling is human work and does not need AI budget. A valid cohort
  can still be evaluated after the budget ends; finalization requires a passing
  result, current evidence and explicit approval.

## 15. Race safety

Finalization uses the existing optimistic concurrency inside one project-locked transaction:

- Each disposition stores `evaluated_revision`.
- Finalization writes the screening event with `expected_revision =
evaluated_revision`, under the same advisory lock and `SELECT … FOR UPDATE`
  path as human screening. A mismatch causes `RevisionConflict` and rolls back
  the entire finalization; no partial set of automation exclusions is committed.
- Any human screening event on the record and stage voids its disposition in
  the same transaction. The `waiting_reports` / queue predicates already
  exclude decided records. Changes to frozen evidence invalidate the cohort
  rather than selectively removing records from its inference.

Example: the AI evaluated revision 8; a person changes the record, so the
revision becomes 9; finalization at revision 8 is rejected. A stale AI result
never overwrites newer human work.

## 16. PRISMA

[PRISMA 2020 provides flow-diagram templates](https://www.prisma-statement.org/prisma-2020-flow-diagram)
for reporting study selection. The event classification below is DeepRef's
proposed mapping to that reporting structure.

PRISMA 2020's "records marked as ineligible by automation tools" counts
**only** title/abstract exclusions written by cohort finalization. These are
screening events by an `ai-first-cohort:*` automation actor. Quarantined,
unfinalized records count as awaiting screening. Voided dispositions count
nothing. Sampled records count as human-screened.

The PRISMA query must classify by event actor and cohort, not by AI
involvement. Assistant-applied decisions are recorded under the user actor
today, and stay human decisions.

## 17. Reproducibility

Reproducibility does **not** mean rerunning the LLM and expecting identical
output: hosted models change even at temperature 0. It means the
**authority decision** is recomputable from stored evidence without any model
call. Per cohort, DeepRef persists:

- cohort membership, close reason and time;
- the routing assignment and evaluated revision per record;
- the sampling frame N_x, the seed, the algorithm and the sample membership;
- reviewer identities, exposure provenance, the blinded labels and their
  timestamps, and adjudications if any;
- R_IU and the records behind it;
- τ, the durable project-wide ordinal j, α_j and exact spending recipe,
  zero-miss minimum n, selected n and planner version, estimator version,
  computed p and pass/fail;
- the identity scheme, the full semantic identity snapshot, the policy
  version, and the protocol version;
- the authority transition, explicit residual-reference attestation, owner
  approval (actor, time, forecast shown) and resulting screening event ids.

The V1 [CSV reproduction tool](../../scripts/reproduce-ai-first-audit.py) independently
recomputes the frame, sample and exact inference without model calls. Active
exports withhold conditional workflow topology and model-call counts to preserve
blinding. A future auditor can then recompute: "given these observations, this
reference rule and this policy, the cohort met the programmed finalization
rule." Recomputation does not establish correctness of human labels or the
scientific validity of the chosen policy. Any provider
revision returned by the API is stored on each AI run (Phase 1). The provider
may expose only a mutable alias, so silent upstream changes are possible.
Continuous monitoring (section 2.4 drift triggers) is the fallback, not
identity.

## 18. Global certification

A platform-wide benchmark is **not** a V1 prerequisite. Later it may provide:

- regression testing across releases;
- provider and model comparison;
- a prior that could reduce local audit burden, for example a Bayesian prior
  on D, which would need its own ADR.

Protocol-specific eligibility performance is never assumed from a general
benchmark. Global evidence is an optimization, never the safety foundation.

## 19. Stage-scoped calibration in the new model

Calibration bundles remain immutable, stage-scoped evidence with a component
identity snapshot (Phase 1). Under this ADR they hold:

- eventual `dual_replacement` evidence, after the joint-error study design
  and margin are accepted (section 3.2);
- optionally, a pre-AI-first pilot. The pilot is informative only, because the
  cohort test (section 7) is what authorizes finalization.

Legacy, stage-less bundles are kept for audit as identity scheme 1. They can
never admit automation. Typed refusal reasons (`StageMismatch`,
`IncompatibleIdentityScheme`, `Stale{components}`) drive the user-facing
explanation of why authority dropped.

## 20. Phase 1 foundations (already implemented)

- **Exposure provenance:**
  - the `ai_opinion_exposures` table, append-only, keyed by project, record
    and stage, with source, optional audience, and AI run, proposal and
    decision ids;
  - availability-time database triggers for suggestions and workflow outputs,
    including records created before the reviewer fetches a page;
  - independence filtering of agreement metrics and the
    `independent_reviewer_pairs` API, using unexposed pairs only;
  - blinding of the activity feed and proposal reads for waiting pairs;
  - a decide-first rule and atomic screening/resolve/exposure transaction on
    the resolve endpoint;
  - exposure provenance in audit exports without repeating verdict contents;
  - recording of intentional exposures (workflow routing output, suggestions,
    reveals).
- **Stage-scoped calibration:**
  - `stage` with structural CHECKs;
  - identity scheme and snapshot columns;
  - legacy rows kept as scheme 1 with no fabricated stage;
  - immutability preserved, plus a trigger that blocks new scheme-1 rows;
  - typed admission refusals, with unsupported schemes rejected before the
    matching-hash fast path and migration 0053 closing SQL CHECK null loopholes;
  - stage-filtered second-review sweep.
- **Identity scheme 2:** see appendix A.

At Phase 1 completion none of the following existed. They are now implemented
in V1, with verification recorded in the acceptance report:

- dispositions;
- AI-first queues;
- probability sampling;
- an authority ledger;
- automatic exclusions;
- PRISMA automation counters.

Phase 1 kept the advisory calibration prerequisite and could still show it
paused. V1 removes that prerequisite for advisory opinions; other definition
admission remains calibrated. No product bundle-creation evaluator is added
by the advisory change.

## 21. Consequences

- AI-first is economically useful only for large title/abstract stages that
  find many relevant records. DeepRef will tell most small reviews not to use
  it. That is intended.
- Two blinded audit judgments per record add substantial work. The forecast
  makes that cost and the residual reference uncertainty visible before opt-in.
- The advisory second reviewer is usable from record one. Methods sections must not describe it as dual
  screening.
- Every authority change is explainable through typed reasons and
  recomputable from stored evidence.
- The V1 migrations add tables for:
  - `ai_screening_dispositions`;
  - cohorts and samples;
  - audit labels;
  - `project_ai_screening_authority`;
  - authority transitions and approvals.
    The `project_ai_autonomy` CHECK would gain `dual_replacement` only after
    its joint-error validation design is accepted. That future policy needs
    its own migration and tests.

## 22. Remaining methodological decisions and selected V1 defaults

1. **τ and α.** V1 selects τ from 0.95 and 0.98 and spends the nominal
   review-wide 0.05 as α_j = 0.05/2^j. Neither value guarantees true recall
   with an imperfect reference. Future alternatives need a new versioned policy.
2. **Advisory opinions.** The expanded implementation instruction selects
   removal of the advisory bundle prerequisite. This does not validate AI
   replacement of a human reviewer.
3. **Reference standard.** Is the hybrid dual OR acceptable? Or should
   adjudicated labels be required for publication-grade claims at a further
   cost of 0.1 judgment per audit record under the scenario disagreement
   assumption? How will sensitivity specifically on AI-excluded records be
   measured before true-recall claims are permitted?
4. **Cohort approval.** V1 requires per-cohort approval and residual-reference
   attestation. Standing finalization is deferred to a separate policy.
5. **Finalized exclusions on amendment.** V1 conservatively reopens
   automation exclusions when the stage criteria change. Narrower reopening
   rules need explicit semantic evidence.
6. **Approver identity.** V1 preserves self-asserted actors and makes the
   attestation limitation explicit. Authentication and proof of distinct
   human reviewers remain requirements for stronger provenance claims.
7. **Minimum abstract length.** V1 requires at least 50 trimmed characters,
   a nonempty title, grounded criteria and two agreed validated exclusions.
   This deterministic eligibility rule is versioned; it is not a confidence score.
8. **Full-text AI-first.** Excluded in V1. Should it be revisited only with a
   different design, such as targeted-reason exclusions?
9. **Recommendation thresholds.** Are 20% and 500 judgments the right
   defaults? Should they be per-project settings?
10. **Human replacement.** Approve the deferred joint-error non-inferiority
    study design and margin before `dual_replacement` becomes available.
11. **Multiple cohorts.** V1 uses durable review-wide alpha spending. Any
    future alternative must preserve conditional test validity, avoid budget
    resets and prohibit repeat sampling until success.
12. **Reference uncertainty.** Which reference-quality assumptions are
    acceptable for automation authorization, and what validation or sensitivity
    correction is required? The current exact-label test alone does not
    guarantee true recall with imperfect labels.

## Appendix A: semantic identity scheme 2 and the component snapshot format

> Superseded: scheme 2 was replaced by the scheme-3 semantic contract
> (`crates/review/src/contract.rs`) in PR0/PR1. The recipe below is kept as
> the historical record; for the live implementation see
> [`contract.rs`](../../crates/review/src/contract.rs).

The actual recipe is implemented in
[`contract.rs`](../../crates/review/src/contract.rs),
[`manifest.rs`](../../crates/review/src/manifest.rs),
[`golden.rs`](../../crates/review/src/golden.rs), and
[`build_support/fingerprint.rs`](../../crates/review/build_support/fingerprint.rs).
A screening snapshot has this shape (hash strings below are descriptive
placeholders, not production digests):

```json
{
  "scheme": 2,
  "definition": "screening",
  "stage": "title_abstract",
  "components": {
    "definition": "sha256-hex",
    "prompt": "sha256-hex",
    "schema": "sha256-hex",
    "policy": "sha256-hex",
    "parser": "sha256-hex",
    "protocol": "sha256-hex",
    "models": "sha256-hex",
    "provider_endpoint": "sha256-hex",
    "golden_render": "sha256-hex",
    "golden_parse": "sha256-hex",
    "implementation": "sha256-hex",
    "dependencies": "sha256-hex"
  }
}
```

The aggregate is SHA-256 of the snapshot's deterministic JSON serialization:
fixed struct field order and a `BTreeMap` for component ordering. This is the
Rust recipe's canonical encoding, not a claim of interoperability with an
arbitrary JSON canonicalization standard. Every hash is validated by
`ReviewHash`; `SemanticIdentity::validate` requires the exact component set,
scheme 2, and a stage exactly when the definition is screening. Non-screening
snapshots have `stage: null` and omit the screening-only golden components.

| Component                              | Actual input                                                                                                                                                                                |
| -------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `definition`                           | Compiled definition id and version, declared-assets hash and workflow hash                                                                                                                  |
| `prompt`, `schema`, `policy`, `parser` | Respective checked-in compiled bundles                                                                                                                                                      |
| `protocol`                             | Subject protocol criteria hash; shared screening protocol changes can invalidate both stages                                                                                                |
| `models`                               | Sorted resolved routes: profile, provider, model, declared version and parameters hash, excluding endpoint                                                                                  |
| `provider_endpoint`                    | Sorted `(profile, optional normalized endpoint)` pairs                                                                                                                                      |
| `golden_render`                        | Hash of deterministic reports from persisted-subject-shaped fixtures through production `prepare_screening_task`, prompts, schema and provider request serialization                        |
| `golden_parse`                         | Hash of interpreted production response outcomes: normalization, semantic validation, proposal payload, independent-screen rule, second-reviewer opinion and workflow verdict, or rejection |
| `implementation`                       | Screening: build-time digest of the declared source boundary. Other definitions retain the broad source-tree digest until their own boundary exists                                         |
| `dependencies`                         | Build-time digest of selected semantic dependency roots, their pruned resolved transitive closure, and manifest declarations including features                                             |

`golden_render` and `golden_parse` are computed once per process on first use
and included in every screening manifest. They use deterministic fixtures at
[`crates/review/golden`](../../crates/review/golden). Fixture failures fail
manifest construction. Production does not read the committed expected
fingerprints as its identity source. The `golden_snapshot_is_current` test
compares runtime outcomes with `screening-fingerprints.json`; intentional
changes must review and refresh that snapshot. The fixtures are finite and
currently fingerprint both stages together, so shared golden or
implementation changes may conservatively invalidate both stages.

The interim screening source boundary includes:

- `review-definitions/`, `crates/review/src/` and `crates/review/golden/`;
- semantic modules in `crates/ai/src/` and `crates/domain/src/`;
- workflow gating and autonomy in `crates/application/src/workflows/`;
- Postgres subject preparation, screening retrieval, proposal acceptance,
  screening state transitions, autonomy application, AI-reviewer logic,
  `ai_exposure.rs`, `activity.rs` and migration 0052 availability triggers;
- worker `processor.rs` and workflow `nodes.rs`.

The V1 extension also includes `crates/application/src/ai_first.rs`, the
`crates/postgres/src/ai_first/` directory and migration 0054;
`crates/review/src/ai_first.rs` is covered by the review source directory.
The declared policy component includes V1 metadata, and golden parse outcomes
include the automation eligibility result. These are requirements for the
selected V1 implementation; final verification remains in the acceptance
roadmap.

The build table records every included path and the reasons for exclusions.
Required missing paths fail the build. Rust `#[cfg(test)]` items and named
test-only files are excluded conservatively. Assistant chat and tools,
classification/deduplication tasks, pricing, usage and offline evaluation
modules are excluded where they are outside screening execution. Worker and
adapter files are included whole, so unrelated changes within those files
still over-invalidate. This is a conservative interim boundary, not a
perfectly isolated screening engine.

Document parsing in `crates/documents/` is outside this boundary: stored blocks
enter through retrieval, and parser changes must change the document parser
version used by `active_parser_version`. HTTP adapters, graph utilities,
non-semantic run persistence, frontend code, docs and standalone tests are
also outside. Reviewers must preserve those exclusions' assumptions as code
moves; the build table is not a proof that every future semantic path is
covered.

The dependency roots currently include `serde`, `serde_json`, `schemars`,
`jsonschema`, `sha2`, `uuid`, `chrono`, `unicode-normalization`,
`unicode_categories` and `rapidfuzz`. The closure records versions, sources and
checksums plus declarations of root features. Transport, runtime and selected
macro/dev dependencies are pruned explicitly. Generated audit listings are
`OUT_DIR/semantic_implementation.txt` and `semantic_dependencies.txt`; the
latter is exposed as `SEMANTIC_DEPENDENCIES`. No whole-lockfile digest is used
for screening calibration.

Endpoint identity is `scheme://host[:nondefault-port][/path]`. Normalization
accepts HTTP(S), lowercases the parsed host, strips trailing path slashes, and
removes username, password, query and fragment. Paths remain because they
select APIs; configuration must not place secrets in path segments. Query
parameters that select a backend are outside this recipe and therefore must
not be used as the only identity of distinct deployments. This is a secret-safe
base-endpoint identity, not a cryptographic identity of the answering model.

The process registers the configured gateway endpoint and the scheduler
stores it on each resolved route. The worker compares recorded and configured
optional endpoints symmetrically before calling the model. A configured worker
rejects a missing scheduler endpoint (including a legacy manifest) as well as
a different endpoint, so an unpinned route is not silently executed. Both
endpoints absent are allowed for unconfigured in-process fixture gateways. Provider
response model identifiers and `system_fingerprint`, when returned, are audit
provenance on the corresponding run, not advance evidence of stable model
weights. Mutable aliases and upstream drift remain possible.

The broad source-tree hash continues in `ReviewRuntimeIdentity.build_sha` for
audit. `rust_version`, `target` and optional `deployment_build_id` are also
audit values and do not change **screening** calibration. The compiler version is the actual compiler `--version` and target is the
Cargo `TARGET` captured by the Postgres build script. Runtime remains in
the full manifest hash and node fingerprints, so changed runtime provenance
can still prevent node/model-call reuse. Non-screening definitions currently
use `build_sha` as their implementation component; the audit-only rule for
that value must not be generalized beyond screening.

Calibration bundles persist `stage`, `identity_scheme`, `identity_snapshot`
and aggregate `semantic_bundle_hash` immutably. Migration 0050 retains legacy
rows as scheme 1 with null stage and snapshot, without updates or fabricated
stage provenance; new inserts cannot use that legacy shape. Database checks
bind snapshot scheme, definition and stage to their columns, with migration
0053 requiring a true shape expression rather than allowing SQL null to pass.
Typed Rust validation enforces the recipe's components. Admission validates
bundle status, scope and supported scheme before aggregate equality as its
fast path, then returns
`Missing`, `Failed`, `StageMismatch`, `IncompatibleIdentityScheme`, or
`Stale { components }`. A changed endpoint is distinguished from changed
model routes. Legacy or differently shaped snapshots are incompatible rather
than assigned a guessed stale component.

The manifest embeds `semantic_identity`; audit exports include that manifest,
so the scheme and snapshot travel with the immutable evidence. Model profiles
are stage-specific, allowing a title/abstract route edit to invalidate its
bundle without changing a full-text route. Shared protocol, source boundary
and golden fingerprints can invalidate both. Phase 1 does not alter that
conservative behavior and adds no scientific authority.

## References

- Callaghan, M. W., & Müller-Hansen, F. (2020). [Statistical stopping criteria
  for automated screening in systematic reviews](https://doi.org/10.1186/s13643-020-01521-4). _Systematic Reviews_, 9, 273.
- Page, M. J., et al. (2021). [The PRISMA 2020 statement](https://doi.org/10.1136/bmj.n71). _BMJ_, 372, n71
  ("records marked as ineligible by automation tools").
- Gartlehner, G., et al. (2020). [Single-reviewer abstract screening missed 13
  percent of relevant studies: a crowd-based, randomized controlled trial](https://doi.org/10.1016/j.jclinepi.2020.01.005).
  _Journal of Clinical Epidemiology_, 121, 20–28.
- Waffenschmidt, S., et al. (2019). [Single screening versus conventional
  double screening for study selection in systematic reviews: a methodological
  systematic review](https://doi.org/10.1186/s12874-019-0782-0). _BMC Medical Research Methodology_, 19, 132.
- Clopper, C. J., & Pearson, E. S. (1934). [The use of confidence or fiducial
  limits illustrated in the case of the binomial](https://doi.org/10.1093/biomet/26.4.404). _Biometrika_, 26(4), 404–413.

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

### V1 additions to the semantic boundary

The boundary also includes AI-first persistence/routing/audit code, the pure
application audit calculation, migrations 0054–0055, masked graph/assistant/full-text reads, public automation inspection and
audit export blinding. The dependency closure includes `num-bigint`, whose pure
integer arithmetic implements exact authorization; it performs no I/O. The
independent CSV reproduction script and test fixtures are verification tools,
not runtime authority components. Source and dependency reports remain
reviewable build outputs.
