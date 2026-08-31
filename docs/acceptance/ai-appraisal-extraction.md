# AI appraisal, extraction, and study-grouping acceptance

These three reviewer flows execute through the compiled `deepref-review` seam. The HTTP and
assistant adapters schedule a checked-in definition, receive `202 Accepted`, and observe the
durable review-run resource until it completes, blocks, or fails. AI output remains proposal-only:
a human reviewer must inspect the evidence and explicitly accept or reject before scientific state
changes.

The fail-closed cases are part of acceptance: stale subject or protocol revisions block
finalization; missing, failed, or stale calibration blocks automation-triggered runs; malformed or
ungrounded model output fails a bounded step attempt; disagreement in screening requires human
adjudication; and retries must reuse only an exact accepted fingerprint and create no duplicate
proposal.

## Study grouping review

- The Studies screen requests a grounded grouping proposal for a report and displays the proposed
  target study (or a new study), report metadata, evidence fields, content hashes, and the
  expected membership revision.
- Accepting sends only the grouping decision and applies the reversible membership change after
  the backend rechecks the target and previous-study revisions. Rejecting sends no reviewed
  payload and changes no membership.
- Stale revision conflicts keep the proposal pending and leave membership unchanged. Accepted
  grouping, study creation, report movement, and review decisions are auditable.

## AI appraisal pre-fill review

- The Appraisal screen pins the report and exact definition/version, loads only matching pending
  pre-fill proposals, and maps every typed answer into the existing generic appraisal form.
- Reviewers can edit answers, domain and overall judgments, and evidence selections. Each evidence
  link preserves the report, page, document block, parser version, and content hash and opens the
  established full-text route.
- Accepting sends a typed `appraisal_prefill` reviewed payload and refreshes completed history;
  the normal manual completion endpoint is not called. Rejecting omits `reviewed_payload`.
- Acceptance is revalidated against the pinned report, definition version, and current evidence.
  A stale proposal remains pending and screening eligibility is never changed by appraisal.

## Typed extraction review

- The Extraction screen selects a study through the `study` URL parameter, shows current
  project-scoped field definitions, and creates only positive integer versions (`version >= 1`).
- Reviewers can edit proposed text, finite number, boolean, or strict ISO-date values, edit
  rationales, and mark optional fields `insufficient_evidence`. Required insufficiency is visibly
  blocked and rejected by validation. An original insufficient field without a source cannot be
  converted locally; the reviewer is told to generate a new grounded proposal.
- A source-backed field can be marked insufficient and then converted through “Enter reviewed
  value”; the draft uses the current field definition to select the typed editor and retains the
  original source as provenance. No evidence is fabricated.
- Every accepted value links report, page, block, document, parser version, and content hash.
  Accepted values refresh only after audited approval; grouping changes do not rewrite their study
  provenance. Rejection sends no `reviewed_payload`, and conflicts keep the proposal pending.

## Endpoints used by the UI

- `GET /api/projects/:projectId/studies?limit=100`
- `POST /api/projects/:projectId/reports/:reportId/ai/study-grouping`
- `GET /api/projects/:projectId/review-runs/:runId`
- `GET /api/projects/:projectId/ai/proposals?status=pending&task_kind=study_grouping&target_report_id=:reportId`
- `POST /api/projects/:projectId/ai/proposals/:proposalId/decision`
- `GET /api/projects/:projectId/appraisal-definitions`
- `GET /api/projects/:projectId/reports/:reportId/appraisals`
- `POST /api/projects/:projectId/reports/:reportId/appraisals`
- `POST /api/projects/:projectId/reports/:reportId/ai/appraisal-prefill`
- `GET /api/projects/:projectId/extraction/fields`
- `POST /api/projects/:projectId/extraction/fields`
- `GET /api/projects/:projectId/studies/:studyId/extraction`
- `POST /api/projects/:projectId/studies/:studyId/ai/extraction`
- `GET /api/projects/:projectId/ai/proposals?status=pending&task_kind=data_extraction&target_study_id=:studyId`
- `POST /api/projects/:projectId/ai/proposals/:proposalId/decision`

## Validation commands

The acceptance gate is:

- `pnpm --filter @deepref/web check` — 0 errors and 0 warnings from `svelte-check`.
- Full Prettier and ESLint pass.
- Vitest passes.
- The production build passes.
- The OpenAPI/Orval drift check passes.
- Focused Studies, Appraisal, and Extraction Playwright tests pass.
- `git diff --check` passed.

The corresponding commands are:

```bash
pnpm --filter @deepref/web check
pnpm --filter @deepref/web lint
pnpm --filter @deepref/web test -- --run
pnpm --filter @deepref/web build
pnpm --filter @deepref/web exec playwright test \
  src/routes/projects/\[projectId\]/studies/page.svelte.e2e.ts \
  src/routes/projects/\[projectId\]/appraisal/page.svelte.e2e.ts \
  src/routes/projects/\[projectId\]/extraction/page.svelte.e2e.ts
bash scripts/check-api-codegen.sh
git diff --check
```
