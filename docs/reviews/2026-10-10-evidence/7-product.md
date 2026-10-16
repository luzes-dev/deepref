# DeepRef whole-application review: product strategy lens

Reviewer 7 of 7. Read-only. Sources: README, docs/prd/*, docs/adr/* (especially ADR 0005), docs/acceptance/*, docs/ui-review.md, docs/ux-walkthrough/relatorio-2026-10-06.html, the handoff docs, docs/rete-workflow-migration.md, docs/openapi.json (166 operations), the route tree and navigation, crates/domain, crates/app/migrations (0001–0058), and targeted reads of the code that implements key capabilities. Every claim below about capabilities cites the code or contract I checked.

---

## 1. Product model

### 1.1 Who the users are, as the product actually encodes them

| Signal in the code | What it says about the user |
|---|---|
| `docs/architecture.md`: "The application has no user/tenant model." `apps/web/src/lib/api/reviewer.ts`: the reviewer ID is a free-text `sessionStorage` value, default `local-user`. It is sent as the `x-actor-id` header (`crates/http-api/src/routes/actor.rs`), and the comment says it "does not authenticate a person". | The real user is **one researcher** on a self-hosted instance. "Reviewers" exist only as labels. |
| Global `/settings` (Crossref mailto, depth, rate limits). The AI provider is configured through the environment only (`AiStatusDto` has no settings path). Setup uses mise and Docker. | The user is **technical enough to self-host**: a methods-literate PhD student, postdoc, guideline methodologist, or research engineer. |
| ADR 0005 runs to 1,453 lines and covers a hypergeometric audit, alpha spending, reference retention and owner attestation. | A second persona is the **methodologist evaluating AI in evidence synthesis**. They are the reason for the deepest engineering investment. |
| The workflow engine has webhooks, inbound email, Slack, HTTP requests and secrets (migration `0028_workflow_engine.sql`, `services/worker/src/workflows/nodes.rs`). | A third persona is implied: the **automation power user**. It does not match the first two. |
| Code volume: screening (web feature plus postgres) is about 5.3k LoC (lines of code). Workflows/automations total about 20k (application 6.0k, postgres 2.4k, http 1.8k, worker 2.4k, web 4.4k, orphaned web `workflows/domain` 2.9k). The `ai` crate is 17k, `review` 8.4k and AI-first 1.8k. | Investment is spread across the personas in proportions that do not match how often each job happens. |

Conclusion: today DeepRef is a **single-researcher, self-hosted SR workspace (SR = systematic review) with research-grade AI-screening governance**. Systematic reviews to the Cochrane/PRISMA standard are team work: two independent screeners, consensus, and a second extractor or verifier. The product cannot represent a team yet, and the AI-first audit needs "two distinct blind human identities". So DeepRef's most rigorous feature depends on a capability it does not have (§3, H1).

### 1.2 Jobs to be done, and how each is weighted

| # | Job | Frequency | Value | Trust sensitivity |
|---|---|---|---|---|
| J1 | Turn a question into a fixed, versioned protocol | once, plus amendments | high | high: prevents post-hoc criteria drift |
| J2 | Gather records from several databases plus citation chasing, and remove duplicates | a few times per review | high | medium: PRISMA counts depend on it |
| J3 | **Decide title/abstract eligibility for hundreds to tens of thousands of records** | **highest**: thousands of decisions | high | high: false exclusions are invisible |
| J4 | Get full texts and decide with a reason | high: tens to hundreds | high | high |
| J5 | Group reports into studies, appraise risk of bias, extract data | medium per study | **very high**: this is the evidence | **very high** |
| J6 | Report: PRISMA 2020, methods text, data for meta-analysis | once or twice | **very high** for publication | high |
| J7 | Use AI to cut workload **and defend it to peer reviewers** | continuous | high | **highest** |
| J8 | Keep the review current (living or update reviews) | periodic | medium to high | medium |

### 1.3 Primary journey as built

Overview (a pipeline that names the next step, `apps/web/src/lib/features/projects/review-progress.ts`) → Protocol (PICO/PICOS/PECO/PEO/PCC/SPIDER/custom, ordered criteria, publish/amend, version diffs) → Imports (DOI with citation depth, PMID, RIS/NBIB/BibTeX/CSV) → Deduplication → Title & abstract (keyboard focus mode, AI suggestion, Conflicts tab, AI-first tab) → Full text (PDF viewer with parsed blocks, one primary exclusion reason, Unpaywall per report) → Studies → Appraisal → Extraction → PRISMA and exports.

Side destinations get equal weight in the navigation: Articles, Graph, Recommendations, Automations, Assistant, AI activity. That makes **16 sidebar items in 5 groups** (`apps/web/src/lib/features/projects/navigation.ts`).

### 1.4 Core value, and whether the UI makes it obvious

The core value is **"AI help you can defend in a methods section."** It rests on:
- every decision being an append-only event with protocol version, actor, revision and supersession (`screening_events`);
- AI output arriving as a typed proposal with criterion-level judgments and block-level evidence (`ai_proposal_criterion_judgments`, `ai_proposal_evidence`);
- blinding plus exposure provenance, so agreement statistics exclude pairs where the human saw the AI first (`ai_opinion_exposures`, and `StageAgreement.excluded_exposed` in `crates/app/src/ai_reviewer.rs:402`);
- immutable compiled run manifests and a deep audit export.

**It is not obvious from the UI.** The defensibility story never reaches a user-facing artifact: there is no methods text, no AI disclosure, and no agreement report on the PRISMA/report page. Meanwhile Graph, Recommendations, Automations and Assistant compete for attention at the same level as screening. The AI setup is split across env vars, global Settings (autonomy levels) and a tab inside screening (AI-first). A new user sees a citation-graph tool with an SR workflow attached, which is how the product grew (`docs/architecture.md` opens "DeepRef maps article citation networks from seed DOIs"). They do not see an SR tool whose distinctive feature is trustworthy AI.

---

## 2. What is good product-wise

1. **Rigorous but quiet screening core.** Keyboard I/E/M/U, a decision bar whose pressed state shows the recorded decision, Undo as a superseding event, and a 409 on stale writes with no silent overwrite. The 2026-10-06 walkthrough judged it "good" with real data.
2. **The protocol is a real scientific object.** Publishing locks a version, amendments create new versions with field-level diffs (`GET …/review/protocol/versions`), and every decision references the protocol version.
3. **AI governance is better than the market.** Suggest, then blinded advisory second reviewer, then AI-first with a fixed probability audit. Each level comes with an explicit "claim a methods section may make" (ADR 0005 §1 table). Screening `act` is forbidden in code (`crates/application/src/workflows/autonomy.rs:108-113`). Human–AI Cohen's kappa excludes exposed pairs. No competitor I know of models exposure provenance.
4. **Evidence identity done right.** Record, Report and Study are separate. DOI is not the key. Resolution never deletes source records, so PRISMA counts reconcile (ADR 0003, PRD 01).
5. **Grounded evidence.** Extraction values require a source block, page, parser version and content hash (`0018_ai_appraisal_extraction.sql`). The PDF parser now emits paragraphs instead of fragments.
6. **Methodologically correct RoB 2.** It uses the official signalling questions plus the algorithm, and the reviewer confirms each judgment (`crates/application/appraisal-definitions/deepref-rct-rob2/v2.json`).
7. **A live, derived PRISMA 2020 diagram** that shows only reasons that occurred. Exports include RIS, BibTeX, CSV, PRISMA JSON/SVG, audit CSV and protocol JSON.
8. **Real UX iteration.** The team ran a real-review walkthrough with 28 findings and fixed 27. The fixes include manual extraction values (no forced AI), protocol autosave, an honest "AI not configured" state, and abstract enrichment from Europe PMC/OpenAlex.
9. **Honest documentation culture.** Acceptance docs separate "verified" from "assumed" and avoid over-claiming, for example: "does not attest that reviewers are distinct people".

---

## 3. Capability gap analysis

Benchmarks: Covidence (Cv), Rayyan (Ry), DistillerSR (DS), EPPI-Reviewer (EPPI), ASReview (AS).

| Capability | Status in DeepRef (evidence) | Market | Importance |
|---|---|---|---|
| **User accounts, project members, roles** | **Missing.** "No user/tenant model" (`docs/architecture.md`). The actor is a self-asserted `x-actor-id` from `sessionStorage` (`apps/web/src/lib/api/reviewer.ts`). The reviewer ID is editable only inside the AI-first panel (`AiFirstPanel.svelte:171`). | All except AS-solo | **Critical** |
| **Dual independent human screening with blinding** | **Missing.** `screening_state` has one status per report per stage (`0006_evidence_workspace.sql:125`). The only "second reviewer" is AI (`ai_reviewer_decisions`). Two blind human labels exist **only** inside AI-first audits (`ai_screening_audit_labels`, `0054`). | Cv, Ry, DS, EPPI | **Critical** for Cochrane-grade |
| Conflict resolution | **Partial:** human vs AI only. `ConflictsPanel.svelte` and `POST …/ai/reviewer-decisions/{id}/resolve`, with resolution enum `kept_human/adopted_ai/other`. | Cv, Ry, DS, EPPI | High |
| Agreement statistics (kappa) | **Partial:** human–AI Cohen's kappa over I/E/M, excluding exposed pairs (`crates/app/src/ai_reviewer.rs:412`). No human–human kappa, no confidence interval or PABAK, and not in any export or report. | Cv, DS, EPPI | High |
| Protocol authoring and versioning | **Has** (`crates/domain/src/protocol.rs`, publish/amend/diff). | Cv (basic), DS | — |
| PROSPERO/OSF registration export | **Missing.** No registration number, outcomes, planned analyses or planned RoB tool fields. Only `protocol.json`. | none native (Cv partial) | Medium |
| **Search strategy documentation (PRISMA-S)** | **Missing.** The file import captures only format and column mapping (`ImportFileForm.svelte`). No database name, platform, date searched or query string. PRD 01 says acquisition runs record "query or import metadata", but the UI doesn't collect it. | DS, EPPI (partial) | **High** |
| Database export import (PubMed/Embase/Scopus/WoS) | **Has:** RIS, NBIB, BibTeX, CSV with column mapping, DOI list, PMID list (`apps/web/src/lib/features/projects/imports.ts:4-25`). EndNote XML and WoS CIW are missing. | all | — / Low |
| Direct database search | **Partial:** a PubMed keyword/author/journal query exists only as an automation "publication alert" trigger (`crates/application/src/workflows/model.rs:201-240`). | EPPI (some) | Medium |
| Deduplication review UI | **Has:** exact auto-merge (default `act`), fuzzy proposals with a comparison view, AI dedupe suggestion, resolution events. The overview presents "Run deduplication (N new records)" as a manual step (`review-progress.ts:90`). | Cv, Ry, DS | — |
| Full-text retrieval | **Partial:** a per-report "Find open-access PDF" via Unpaywall (`FullTextScreening.svelte:873`). Bulk retrieval is possible only by building an automation (`action.attach_pdf`, `nodes.rs:971`). No link resolver or ILL tracking. | Cv (bulk), EPPI | High |
| PDF parsing and anchored evidence | **Has:** pdfium blocks with coordinates, sections and references (`0026`, `0030`), GROBID crate. | DS (partial) | — (differentiator) |
| **Risk-of-bias tools** | **Partial:** only RoB 2 (official), a generic RCT tool and a generic qualitative tool (`crates/application/appraisal-definitions/`). Study classification **suggests ROBINS-I, ROBINS-E, QUADAS, JBI, CASP, AMSTAR 2** (`crates/domain/src/bibliography/study.rs:161-204`), none of which exist. Appraisal is keyed **per report** (`appraisal_assessments.report_id`), but RoB 2 is per result/outcome and extraction is per study. No dual appraisal or consensus, no robvis plot. | Cv (RoB 1/2 + custom), DS (any) | **High** |
| GRADE / summary of findings | **Missing.** | Cv (basic), GRADEpro | Medium to high |
| **Structured data extraction** | **Partial:** flat study-level fields of type text, number, boolean or date (`extraction_field_definitions`). One value per study per field. Values carry a source citation, history, an AI proposal path and a verification flag. No arms, outcomes or timepoints, no choice/categorical type, no dual extraction or consensus, no templates. | Cv, DS, EPPI | **High** |
| Meta-analysis-ready export (RevMan / R metafor) | **Partial:** `extraction.csv` is long format, one row per value (`crates/http-api/src/routes/exports/review_tables.rs:261`). No wide study×outcome×arm table and no effect-size fields. | Cv (RevMan), DS, EPPI | Medium to high |
| **PRISMA 2020 flow diagram** | **Has, with gaps.** Standard diagram plus SVG/PNG/JSON. Identification is a **single column** (`crates/app/src/prisma.rs:30-37`, `apps/web/src/lib/features/prisma/flow.ts`): no per-database n, no "other methods" column (citation searching, which is exactly what DOI traversal is), no registers, no update-review variant. AI-first exclusions are counted correctly (ADR 0005 §16). | Cv, DS | **High** |
| PRISMA checklist / methods text | **Missing.** | none | High (differentiator) |
| Living review / search updates | **Partial:** "Weekly literature alert" templates (`crates/application/src/workflows/templates.rs:158-206`). `POST …/acquisitions/{id}/refresh` works only for Crossref citation traversal and rejects file imports (`crates/http-api/src/routes/acquisitions.rs:302-325`). The search is not tied to the protocol, and PRISMA has no update cycles. | DS, EPPI (some) | Medium |
| Audit trail export | **Has, and it is deep:** screening, study and appraisal events, AI runs, manifests, step attempts, artifacts and lineage, cohorts, labels, exposures and calibration (`crates/app/src/audit_export.rs`). But the CSV has raw JSON snapshot columns (`exports.rs:423-444`), it is not human-readable, and nothing summarises it. | DS (audit), others weak | High (differentiator) |
| **Screening prioritisation (active learning)** | **Missing.** Queue sort is only created, title or year (`crates/app/src/screening.rs:855-863`). | Ry, Cv, DS, EPPI, AS (core) | **High** |
| AI screening assistance | **Has, best in class on governance** (suggest, blinded second reviewer, AI-first with audit). | DS, Ry (newer), others | — (differentiator) |
| Keyword highlighting in the reader | **Missing** (no highlight in `features/screening`). | Ry, Cv | Medium (small effort) |
| Record labels and tags | **Missing.** | Ry, Cv | Medium |
| Project export/import (backup, handover) | **Missing** (no endpoint). | DS, EPPI | Medium |
| Citation chasing (snowballing) | **Has:** backward via Crossref references with depth, plus a graph. Forward citation not verified. | none native (Citationchaser separate) | Differentiator if framed as a search method |

### What "no user/tenant model" means for the product

- **Methods claims are unprovable.** ADR 0005 §2.3 says it outright: "Actor identity is self-asserted today (`x-actor-id`), so the approval is an attestation, not authentication." The AI-first audit requires "fresh independent people", but those are enforced only as distinct strings. One person in two browser tabs satisfies it.
- **The gold-standard workflow cannot be represented.** Dual independent screening, consensus and calibration exercises among humans are impossible, and so is a supervisor/student split, which is the normal Cochrane review-team setup.
- **Hosted deployment is impossible**, because any visitor can act as anyone. That limits DeepRef to self-hosting by technical users, a much smaller market.
- **There is a mismatch.** The engineering invested in proving AI is safe exceeds the engineering invested in proving **who** did what. Reviewers of a manuscript will question the second before the first.

---

## 4. Ranked product opportunities

Each item condenses nine points: **Problem** · **Why now** (what in the current system makes it relevant) · **Who benefits** · **Value** · **Product complexity (PC)** · **Engineering complexity (EC)** · **Architecture fit** · **Weakness exposed** · **Smaller version**.

### 4.1 High-confidence improvements

**H1. Reviewer identity and a project roster (the basis for teams)** (Rank 1)
- **Problem:** Two people cannot be told apart, and AI-first audits and approvals rest on self-asserted strings.
- **Why now:** Every event table already has `actor_kind` and `actor_id`. The queue has a `reviewer_id` filter. AI-first enforces distinct actors and exposure exclusions. Only a trustworthy source of `actor_id` is missing.
- **Who benefits:** Every team review, and anyone publishing AI-first results.
- **Value:** Very high, because it is the precondition for H2, S2 and credible AI claims.
- **PC:** Medium (sign-in, a member list, a reviewer picker).
- **EC:** High for full auth. Medium for the smaller version.
- **Architecture fit:** Good on the data side. On the HTTP side, the actor middleware (`routes/actor.rs`) is the single seam to swap.
- **Weakness exposed:** Global settings and the env-only AI configuration would need project or tenant scoping later.
- **Smaller version:** A project roster of named reviewers plus trusted reverse-proxy identity: accept an `X-Forwarded-User` from oauth2-proxy or Authelia, configured on, and refuse the free-text header when it is set. This gives self-hosted teams real identities without building auth.

**H2. Human dual screening, built by generalising the AI second reviewer** (Rank 2)
- **Problem:** Cochrane and most journals expect two independent screeners plus consensus. DeepRef offers one human, optionally with AI.
- **Why now:** The hard UX already exists for human-vs-AI: blind-until-decided reveal, exposure provenance, the Conflicts tab with a count badge, a resolve endpoint, kappa.
- **Who benefits:** Teams, and any review submitted to Cochrane or a high-tier journal.
- **Value:** Very high; this is table stakes against Covidence and Rayyan.
- **PC:** Medium. Add a "screening mode" per stage: single, dual human, or human plus blinded AI.
- **EC:** Medium to high. Add per-reviewer opinions, with `screening_state` becoming the consensus projection. PRISMA and downstream steps are unaffected because they read final state.
- **Architecture fit:** Natural. It is an append-only event log plus a projection, and the reveal and exposure logic is reusable.
- **Weakness exposed:** `ai_reviewer_decisions` is AI-specific (`source IN ('ai','workflow')`, `resolution IN ('kept_human','adopted_ai','other')`; `0029`). "Second reviewer" was modelled as an AI concept rather than a role.
- **Smaller version:** Let a second **human** opinion land in a generalised `reviewer_opinions` table and reuse the Conflicts panel and kappa. Skip roles and assignment at first.

**H3. Search documentation and source-aware PRISMA identification** (Rank 3)
- **Problem:** PRISMA 2020 items 6–7 and PRISMA-S need each database with n, platform, date and full strategy. The diagram needs separate "databases/registers" and "other methods" columns.
- **Why now:** `acquisition_runs` and `record_provenance` already store source and strategy per record. DOI citation traversal is literally PRISMA's "citation searching". The PRISMA projection just doesn't group by them (`prisma.rs:30-37`).
- **Who benefits:** Every published review.
- **Value:** High; manual reconstruction is error-prone.
- **PC:** Low. Add three optional fields on import: database/platform, date searched, query text.
- **EC:** Low to medium (config on the acquisition, a GROUP BY in the projection, a second diagram column).
- **Architecture fit:** Natural.
- **Weakness exposed:** The two import pipelines (legacy `/ingestions` for DOI vs `/projects/{id}/imports` for files; the DOI form calls `createIngestion`) complicate a uniform per-source count.
- **Smaller version:** A database label plus date on file imports, and per-label counts in the identification box.

**H4. Prioritised title/abstract screening** (Rank 4)
- **Problem:** Title/abstract screening is the highest-volume job. Every major competitor surfaces likely includes first, so teams find relevant records early and can plan stopping.
- **Why now:** The queue already has a sort parameter, and the cursor SQL is designed for new sort keys. The worker has durable jobs. A non-LLM model would be deterministic and auditable, which fits DeepRef's identity.
- **Who benefits:** Every reviewer with more than about 1,000 records.
- **Value:** High.
- **PC:** Low (a "Most likely relevant first" sort plus a visible note about which ordering was used).
- **EC:** Medium. Recompute a TF-IDF plus logistic regression model on current decisions in a worker job, and store score snapshots for audit.
- **Architecture fit:** Good.
- **Weakness exposed:** Ordering is a form of exposure; it should be recorded like `ai_opinion_exposures` so methods text can state it.
- **Smaller version:** Sort by the existing blinded AI second-reviewer opinion. It is already computed by default (screening autonomy defaults to `second_reviewer`, `autonomy.rs:101-104`), but it reveals AI information before the decision, which breaks blinding. So the real model is preferable.
- **Caution:** Never let prioritisation auto-exclude. See E2 on stopping rules.

**H5. Bulk open-access PDF retrieval from the full-text queue** (Rank 5)
- **Problem:** Collecting PDFs is the biggest manual chore between the two screening stages.
- **Why now:** The Unpaywall client (`crates/crossref/src/unpaywall.rs`), the per-report action and the bulk `action.attach_pdf` node already exist, but bulk retrieval is reachable only by building an automation.
- **Who benefits:** Everyone.
- **Value:** Medium to high.
- **PC:** Low. EC: Low (enqueue one job, then show progress and "N found / M still missing" in the queue).
- **Architecture fit:** Natural.
- **Smaller version:** This is already the small version.

**H6. Appraisal coverage and unit of assessment** (Rank 6)
- **Problem:** Studies suggests tools that don't exist (ROBINS-I, QUADAS, JBI, CASP, AMSTAR 2, NOS; `study.rs:161-204`), a dead end the walkthrough flagged (F23). RoB 2 is per result, but appraisal is keyed per report.
- **Why now:** Definitions are schema-driven JSON (`appraisal-definitions/*/vN.json`) with versioning, so adding a tool is mostly content work. The RoB 2 algorithm pattern (`suggestion/rob2_official.rs`) shows how algorithmic tools plug in.
- **Who benefits:** Every review with non-RCT or diagnostic studies, which is most of them.
- **Value:** High.
- **PC:** Low per tool. Medium for "per study × outcome".
- **EC:** Low per JSON tool. Medium to re-key appraisal to the study (and optionally the outcome).
- **Weakness exposed:** The three phases use different units: screening per report, appraisal per report, extraction per study.
- **Smaller version:** Ship ROBINS-I v2 and QUADAS-2 definitions, only suggest tools that exist, and add a robvis-compatible CSV (study, domain judgments, overall).

**H7. Extraction that can feed a meta-analysis** (Rank 7)
- **Problem:** Meta-analysis needs per-arm or per-outcome numbers (n, mean, SD, events) and categorical fields. Flat study-level text and number fields plus a long CSV force spreadsheet rework.
- **Why now:** Values already carry provenance, history and verification. AI extraction already proposes grounded values.
- **Who benefits:** Intervention and diagnostic reviews that pool data.
- **Value:** Medium to high.
- **PC:** Medium.
- **EC:** Medium for a choice type and a wide export. High for repeating arm and outcome groups.
- **Architecture fit:** The `value_type` enum and `UNIQUE(project_id, study_id, field_definition_id, version)` assume one value per study per field, so repeating groups break that constraint.
- **Smaller version:** A `choice` field type, field order and grouping, and an `extraction_wide.csv` with one row per study and one column per field. Users then name fields like `outcome1_armA_mean`. This is crude but unblocks R/RevMan import today.

### 4.2 Strategic opportunities

**S1. A "Methods & reproducibility package": DeepRef's signature artifact** (top strategic bet)
- **Problem:** Authors must write methods, a PRISMA flow, search documentation and, increasingly, an **AI-use disclosure**. Emerging guidance on AI in evidence synthesis (for example the RAISE recommendations) and journal policies ask exactly what DeepRef already records. Peer reviewers ask "how was AI used, and how do you know it didn't drop studies?"
- **Why now:** ADR 0005 §1 already writes the allowed methods sentence for each workflow. The audit export joins manifests, prompt and schema versions, model and provider revision, exposures, cohorts (with α_j, τ, n, k) and kappa. Manifests are immutable and recomputable without model calls (ADR 0005 §17).
- **Who benefits:** Every author. It is also the clearest expression of the brand promise "AI without giving up traceability".
- **Value:** Very high, and a differentiator no competitor has.
- **PC:** Low (one "Export methods package" action on the PRISMA/Report page).
- **EC:** Medium. Generate templated methods text from stored settings and statistics, and bundle a zip: protocol versions with diffs, search log (H3), PRISMA SVG and JSON, included studies, extraction, appraisal, AI disclosure table, agreement stats, cohort results, `audit.csv` and the manifests.
- **Architecture fit:** Natural; it is mostly a projection over existing tables.
- **Weakness exposed:** Without H1, the package has to state that reviewer identities are self-asserted. That is honest, but it weakens the product.
- **Smaller version:** A generated methods paragraph plus an AI-disclosure table (model, prompt versions, autonomy level per task, blinding, kappa, n of AI-entered values verified), shown on the PRISMA page with copy and download.

**S2. Make "screening mode" the visible centre of AI value**
- **Problem:** The AI options are scattered: autonomy levels in global Settings, AI-first in a screening tab, Conflicts in another tab, AI activity in Operate. Users cannot see the methodological consequence of each choice.
- **Proposal:** At protocol publish, or per stage, offer a choice of modes:
  - single reviewer;
  - two reviewers (H2);
  - one reviewer plus a blinded AI check;
  - AI-first for large reviews (gated by the ADR §10 threshold of ≥20% and ≥500 judgments saved).

  Show the methods sentence each mode allows, the forecast workload and the cost budget.
- **Why now:** All four mechanisms except dual human already exist, and the ADR §1 table is the copy.
- **Value:** High; it makes the core value legible.
- **PC:** Medium. EC: Low to medium (mostly UI over existing endpoints `GET/PUT …/ai/autonomy`, `…/ai/first`).
- **Smaller version:** Move autonomy settings for screening out of global Settings into a "How this stage is screened" header on each screening page.

**S3. Living review mode, tied to the protocol's search strategy**
- **Problem:** Reviews go stale, and update reviews re-run searches and need PRISMA "previous studies plus new".
- **Why now:** The pieces exist: a publication-alert trigger with a PubMed client and NCBI pacing, idempotent imports, dedupe against the existing corpus, AI second-reviewer pre-screening, and a scheduler.
- **Weakness exposed:** The search lives inside an automation's node config, not in the protocol. Refresh supports only Crossref traversal (`acquisitions.rs:302-325`). PRISMA has no cycle dimension.
- **Value:** Medium to high for guideline groups and living-evidence teams.
- **PC:** Medium. EC: Medium to high.
- **Smaller version:** "Saved searches" (H3 data) with "Re-run now" for PubMed, plus a "new since last update" filter in screening.

**S4. Decide the deployment model (self-hosted single user vs hosted multi-user)**
- Every team feature (H1, H2) and the AI cost model (budget per project, env-configured provider) depend on this.
- A hosted, multi-user deployment changes the security posture of the workflow engine: webhooks, inbound email and arbitrary HTTP requests become multi-tenant SSRF and secrets risks.
- Recommendation: decide this explicitly before investing more in automations.

### 4.3 Small, high-leverage changes

| Change | Why | Effort | Evidence |
|---|---|---|---|
| Highlight inclusion and exclusion keywords in the title/abstract reader (from the criteria, plus user term lists) | A Rayyan staple that speeds the highest-volume job | Low (client-side) | none in `features/screening` |
| Database, date and query fields on the file import form | Unlocks H3 and S1 | Low | `ImportFileForm.svelte` |
| "Find open-access PDFs for N missing" button | H5 | Low | `FullTextScreening.svelte:873` |
| Wide extraction CSV (pivot of the existing long data) | Gets data into R and RevMan | Low | `review_tables.rs:261` |
| Suggest only appraisal tools that exist | Removes a dead end | Very low | `study.rs:161-204` |
| Default citation depth 0 for SR projects, with a volume estimate before expanding | The walkthrough (F16) recommended depth 0. The default is still 1 (`0048_settings_default_depth.sql`), which silently adds every reference of every seed to screening. | Low | migration 0048 |
| Show human–AI kappa and the exposure-excluded count on the PRISMA/report page, not only in the Conflicts tab | Surfaces the core value | Low | `conflicts.ts` |
| Run fuzzy dedup automatically after each import instead of making it a pipeline step | The step exists for architecture, not user need | Low to medium | `review-progress.ts:90` |
| Rename leaked jargon: "Ingestions" → Imports; "Proposal" → "AI suggestion"; AI-first copy ("cohort", "Draw fixed audit", "Evaluate once", "Human reference retention, conditional on the reference labels") → plain language with the statistics behind a disclosure | Leaked concepts (see 4.6) | Low | `AiFirstPanel.svelte:157-428` |
| Fix the known gap where the studies list shows no member counts | Listed twice as a known gap in `docs/ui-review.md` | Low | `GET /projects/{id}/studies` returns `reports: []` |
| Labels and tags on records | Rayyan and Covidence parity, useful for "background" and "check refs" | Low to medium | none |

### 4.4 Product simplifications (remove, merge, hide)

**P1. Shrink the general-purpose workflow engine to purpose-built automations**
- About 20k LoC: a node builder (`@deepref/ui/flow`), versions, publish, test runs, webhooks, inbound email, Slack, HTTP requests and secrets. That is roughly 4× the screening code.
- The SR jobs it actually serves are few: literature alerts, AI pre-screening of new records, PDF retrieval, and "notify me when included".
- The product has oscillated. The walkthrough (F10) said to replace the canvas with "When ⟨event⟩, run ⟨recipe⟩". `ui-review.md` records that the canvas was removed. `assistant-and-workflows-handoff.md` then notes it was replaced by a Svelte Flow builder backed by a full engine (`0028`, `0031`).
- **Recommendation:** Keep the engine internally. Expose 4–5 toggles or templates as forms. Move the canvas behind "Advanced". Park the Slack, HTTP-request, webhook and inbound-email nodes until users ask, which also removes the secrets and SSRF surface.
- The `action.record_decision` node has an `Act` branch that writes screening events (`nodes.rs:795+`). It is unreachable because screening disallows `act`, but it shows how far the engine reaches into scientific state.

**P2. Remove or park calibration bundles**
- `insert_review_calibration_bundle` (`crates/app/src/review_calibration.rs:192`) has **no non-test caller**.
- The `ReviewOrigin::AutomationTriggered { calibration_bundle_id }` admission path has **no production caller** (only `ReviewerRequested`, `AdvisoryTriggered` and `AiFirstTriggered` are used).
- ADR 0005 states "There is no product calibration-bundle creation flow."
- Yet they carry migrations 0022, 0050, 0053 and 0056, a deployment recalibration runbook (`docs/deployment-recalibration.md`), and audit-export joins.
- This is pure complexity with no user path. Delete it, or keep only the schema with a note that it is parked.

**P3. Recommendations: repurpose or merge**
- The page ranks the project's **own** citation network into three top-5 lists: foundational, core, underexplored (`crates/http-api/src/routes/articles.rs:283-360`).
- For an SR, re-ranking records already in the corpus duplicates screening and nudges attention, which is a subtle bias.
- **Recommendation:** Either merge it into Graph as a lens, or repurpose it as **citation-chasing candidates**: works cited by ≥k included studies that are not yet in the project. That is a recognised supplementary search method that feeds PRISMA "other methods" (H3).

**P4. Reposition Graph from "Analyze" to "Collect → Citation searching"**
- As analysis it has low SR value. As a visual for backward and forward snowballing from included studies it supports a real method.

**P5. The assistant: narrow its job**
- It has 16 tools (`crates/ai/src/assistant.rs`), 7 of them `propose_*` or `trigger_workflow`, which duplicate in-context AI buttons and automations.
- A large durable-run stack (Rig, cassette replay, run events; migrations 0057–0058, ADR 0006) supports a feature the walkthrough could not even exercise.
- **Recommendation:** Position it as "ask questions of your corpus with citations" (read tools). Route any proposal it creates to the same queues and keep it off the critical path. Don't add more tools until usage justifies it.

**P6. Unify the two import pipelines**
- DOI imports use legacy `/ingestions` (works, project_works, ingestion_items). File and PMID imports use `/projects/{id}/imports` and acquisitions (records, reports).
- Users see one "Imports" page with two different run models and inspectors (`ImportDoiForm` calls `createIngestion`; `ImportFileForm` calls `importProjectRecords`).

**P7. Collapse the navigation from 16 items to about 8**
- Overview · Protocol · Collect (Imports + Duplicates + Articles + Citation searching) · Screen (TA, FT; Conflicts and AI-first as tabs) · Studies & data (Studies, Appraisal, Extraction) · Report (PRISMA, exports, methods package) · Assistant · Activity (merges AI activity, automation runs and audit).
- Move Automations into project settings or Activity.

**P8. Remove dead or stale artifacts**
- `apps/web/src/lib/features/workflows/domain` (2.9k LoC from the Rete era) is used only by the assistant's workflow preview.
- `docs/rete-workflow-migration.md` still says "Implementation and review are in progress" for a removed editor.
- `docs/architecture.md`'s first sentence describes the old product ("maps article citation networks from seed DOIs").

**P9. Align the autonomy defaults with the brand promise**
- Extraction defaults to `act` (`autonomy.rs:101`). AI writes cited values directly, flagged `needs_verification` (`ai_autonomy_apply.rs:348-384`).
- That is a defensible "AI extracts, human verifies" design, but it contradicts the README's "Its output is treated as a proposal, not as scientific truth."
- **Recommendation:** Either default to `suggest`, or rename the mode honestly ("AI fills, you verify") and block export and PRISMA completion while unverified AI values remain.

### 4.5 Explorations

- **E1. Gate AI-first behind scale.**
  - ADR 0005 §10 recommends AI-first only when it saves ≥20% **and** ≥500 judgments.
  - Each cohort needs two fresh blind humans who did not screen the human stratum and did not approve routing, plus controls (2N_c extra labels).
  - For a solo user with no identity model this is unreachable in practice. Explore hiding the tab until the unscreened count passes a threshold, with a clear "allowed but unlikely to help" message.
  - Validate with users whether anyone reaches that scale.
- **E2. Reuse the hypergeometric audit for stopping rules.**
  - With prioritised screening (H4), the hard methodological question is when to stop. DeepRef's cohort, sample, label and one-look machinery (`crates/app/src/ai_first/`) is close to what a stopping-rule audit needs (screen a random sample of the unscreened remainder and bound the missed includes).
  - That is simpler and more widely applicable than quarantine-based AI-first, and it would make DeepRef the defensible ASReview-style tool.
- **E3. Draft a PROSPERO/OSF registration from the structured protocol**, with AI drafting the free-text fields under the proposal model.
- **E4. Cross-project reuse:** libraries of exclusion reasons, extraction templates and custom appraisal tools (user-defined JSON definitions are architecturally natural).
- **E5. Evidence by text selection in the PDF** (walkthrough recommendation) for appraisal and extraction. Blocks now have coordinates, so map the selection to blocks.
- **E6. Human–human calibration exercises** (pilot screening of 50 records by everyone, kappa before starting). This is cheap once H1 and H2 exist, and DS/Covidence users value it.

### 4.6 Concepts leaked to users, and steps that exist for architecture

- **Leaked vocabulary:**
  - "Ingestions" vs "Imports" vs "Acquisitions" (three names for one job).
  - "Proposal" (should be AI suggestion).
  - "Review run".
  - "cohort", "reference retention", "Draw fixed audit", "Evaluate once", "Close cohort" (AI-first panel).
  - "metrics status" and projection freshness on Articles and Graph.
  - The template "Refresh project statistics" / `project_maintenance`. A user should never schedule metric recomputation (`templates.rs:71`).
  - `POST /metrics/recompute` exposed as a user action.
- **Steps that exist for the architecture, not the user:**
  - running deduplication manually after import;
  - recomputing metrics;
  - "Retry processing" or reparse for PDFs (acceptable as error recovery);
  - recalibration after deploys (`docs/deployment-recalibration.md`);
  - entering a "reviewer ID" by hand;
  - the global Crossref mailto prerequisite (walkthrough F01, still open: "will come from user accounts").
- **Statistically necessary, but needs better framing:** the AI-first close → draw → label → evaluate → approve sequence. It is required for validity (one draw, one look), but should read as a guided wizard with plain-language consequences.

---

## 5. Open questions that need user research

1. **Who is the target user: solo self-hoster or review team?** How many real users screen with a second person today, and how do they do it outside DeepRef (spreadsheets, Rayyan)? This decides whether H1 and H2 come before everything else.
2. **Scale:** what record counts do target reviews reach at title/abstract? Below about 3,000, AI-first's economics (ADR §9–10) rarely pay off, and prioritisation (H4) matters more.
3. **What do journals and peer reviewers in the target fields actually demand** for AI-use disclosure? Would the methods package (S1) be adopted as-is, or must it map onto a specific guideline?
4. **Is the blinded AI second reviewer accepted** by methodologists as a replacement for, or only a supplement to, human dual screening in the users' fields? ADR 0005 correctly refuses the replacement claim. Do users understand that?
5. **Do any users build custom automations** beyond the templates? Which nodes are used? This decides P1.
6. **Do users open Graph and Recommendations** during an SR, and at which stage? Collect is the hypothesis, not Analyze.
7. **Which appraisal tools and extraction shapes** cover 80% of target reviews (RoB 2 + ROBINS-I + QUADAS-2? arm-level outcomes?).
8. **How do users hand data to analysis today:** RevMan, R metafor, Stata, Excel? This decides the export shape for H7.
9. **Deployment:** would target users accept a hosted service with their unpublished review data and AI provider keys, or is self-hosting a hard requirement (institutional data policies)?
10. **Living reviews:** what share of users run update searches, and how often?
