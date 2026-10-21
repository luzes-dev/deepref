> Historical review. This document describes the pre-SaaS architecture and is not normative documentation.

# Reviewer 4: Frontend architecture, state, data fetching, UX, accessibility, responsive, design system

Scope: `apps/web` (SvelteKit 3 SPA, Svelte 5 runes, TanStack svelte-query 6, orval client) and `packages/ui` at `main` b0ad219. This was a read-only review.

**Runtime evidence and its limits:**
- The API was not running on :8080 or :8083. Only the review Postgres on :5443 was listening, so I did a static review. I did not start any services.
- I used the committed Playwright visual baselines (`apps/web/tests/visual/__snapshots__/**`, regenerated 2026-10-08) as screenshot evidence for 390px and 1440px.
- `pnpm --filter @deepref/web build` failed with `ENOSPC`, because `/tmp` (tmpfs) was 98% full during the run. The bundle analysis below therefore uses the existing untracked `apps/web/build/` from 2026-10-08 22:57. That build predates only the assistant-observer commits.
- Baselines supplied by the coordinator: `pnpm check` 0 errors, `pnpm lint` 0 errors and 62–63 `shadcn/no-restyle` warnings (I counted 62 plus 1 `require-static-classes`), vitest web 535 and ui 26.
- I did not repeat what the product reviewer already covered: the 16-item sidebar, jargon, and the reviewer-ID UI.

---

## 1. Frontend mental model

### Route tree (`apps/web/src/routes`)

```
/                         -> ProjectWorkspace (no children): redirects to first project overview
/settings                 -> SettingsView (also opens as a shallow-routed modal: SettingsDialog in root layout)
/projects/[projectId]     -> +page.ts redirect -> /overview
  +layout.svelte          -> ProjectWorkspace (shell + data owner for projects/articles/ingestions)
  overview | protocol | prisma | articles | graph | recommendations
  discovery/imports | discovery/duplicates  (deduplication/ -> +page.ts redirect; its +page.svelte is unreachable)
  screening/title-abstract | screening/full-text   (both wrapped in ai-autonomy ScreeningViewSwitch: "Screening | Conflicts")
  studies | appraisal | extraction
  automations | automations/[workflowId] (xyflow builder)
  assistant | activity
/demo, /demo/playwright   -> scaffold leftovers
```

Every route `+page.svelte` is a one-line mount of a feature component in `src/lib/features/**`. Nothing uses route `load`, and `ssr = false`. All data fetching happens in components via svelte-query.

### State ownership

| State | Owner | Notes |
|---|---|---|
| Project list, selected project, article pages (50/page infinite), **global** ingestions (2 s poll) | `ProjectWorkspace.svelte` (project layout) → pushed into `ProjectWorkspaceContext` as getter "data sources" (`context.svelte.ts:99-160`) | Mounted for *every* project route. It is a god-context: data, URL filters, navigation verbs, dialog flags and draft import depth in one class. |
| Selected article/ingestion/acquisition, article filters/sort, graph filters/overlays | URL search params, through getters and setters on the context (`context.svelte.ts:184-262`) | Good. The URL is the source of truth, but see F11 about leakage across views. |
| Screening position, status filter, search, sort, mode | URL (`screening/filters.ts`, `ScreeningFocus.svelte:78-104`) | Deep-linkable, and an e2e test covers refresh and history. |
| Full-text report, filter, page, block | URL plus `page.state` (`FullTextScreening.svelte:118-121`) | Deep-linkable. |
| Server data per feature | Component-local `create*Query` with generated hooks, mostly via orval-generated keys | Exceptions: TA queue uses a hand key `['screening-queue', projectId, status, search, sort]`; AI activity uses `['ai-activity',…]`; the assistant bypasses svelte-query entirely (F9). |
| Optimistic screening | `screening/optimistic.ts` and `full-text/optimistic.ts` do manual multi-page cache surgery, plus a full queue invalidation after each decision | The handoff's §4.5 simplification has not been done. |
| Browser storage | Pane sizes and nav collapse (ok); protocol draft/amendment (localStorage, intentional); DOI depth; notification watermark; **automation canvas notes** (`AutomationBuilder.svelte:268-303`); reviewer ID (sessionStorage) | |

### Data flow

orval (`orval.config.ts`) generates tag-split svelte-query hooks over `customFetch` (`api/custom-fetch.ts`).
- The fetch wrapper adds actor and correlation headers and throws `ApiError` with status, code and body.
- `forceSuccessResponse` gives `{data,status,headers}` envelopes.
- QueryClient defaults (`api/query-client.ts`): `staleTime 30s`, retry once on 5xx or network failure, `refetchOnWindowFocus`.
- Mutation-to-query invalidation is declared in `orval.config.ts` (`mutationInvalidates`). Most feature code also does `await xQuery.refetch()` locally after a mutation, rather than invalidating by key.

Hand-written client code sits beside the generated client:
- `features/assistant/chat-api.ts`: conversations, messages, plans and budget, with its own DTO interfaces.
- `api/assistant-stream.ts`: SSE/run observer.
- `api/pagination.ts`: cursor helpers for projects and ingestions.
- `features/*/api.ts`: small error predicates such as `isConflict`.

**Polling inventory:**

| Interval | What polls |
|---|---|
| 10 s | Notifications watcher, unread-count bell, and dependency health (`DependencyHealthIndicator.svelte:14-21`), all with `refetchIntervalInBackground:false` |
| 2 s | Global ingestions while any are active; inspectors; PMID runs |
| 2 s for up to 60 s | Articles while refreshing metrics |
| 15 s | Full-text queue (whole infinite query), missing list, and history |
| 5 s | Full-text documents and document, unconditionally |
| 5 s | Automations list |
| 10 s | AI-first panel; assistant queued plans (manual `setInterval`) |
| Custom `setInterval` | Automation run watcher |

---

## 2. What is good

- **The URL is the state for navigation-heavy surfaces.** Screening position, filters, sort, mode, full-text page and block, article filters, graph overlays and settings shallow routing are all deep-linkable. E2E tests check refresh and back/forward (`title-abstract/page.svelte.e2e.ts` "preserves mode, status, sort, and report through refresh and history navigation").
- **Screening keyboard workflow exists and is careful.** I/E/M/U and arrows, plus 1–9 for exclusion reasons in full text (`screening/shortcuts.ts`). Shortcuts are suppressed in fields and open overlays and ignore Ctrl/Cmd/Alt. Pressed-state `aria-pressed` buttons mirror the recorded decision. A visible key legend is shown. `goto(..., {reset:false})` keeps focus when advancing.
- **409 `screening_revision_conflict` is handled explicitly.** The authoritative `details.currentState` is validated (`ScreeningFocus.svelte:242-266`), the cache is reconciled, the user is moved to the "All" filter, and a plain-language message is shown. Both stages have e2e tests for it.
- **The error boundary is consistent.** `ApiError` carries status, code, request and correlation IDs and Retry-After. The retry policy does not retry 4xx.
- **Heavy libraries are mostly lazy.** pdf.js is loaded via `import('pdfjs-dist')`, with its 1.26 MB worker as a separate asset. sigma and graphology load via dynamic `import()` in `project-graph-renderer.ts:928`.
- **Design-system enforcement is real.** `@shadcn/lint` reports 0 errors. There are no `style="..."` attributes in app code, and I found only one raw palette class (`PdfPage.svelte:54 bg-white`, intentional for paper). Package boundaries are enforced with `no-restricted-imports`.
- **Accessibility groundwork:**
  - A skip link in `PageFrame`.
  - Named landmarks (`aria-label` on asides and regions).
  - `role=progressbar` with values for screening progress.
  - Dialog focus trap and restore, covered by tests (`tests/settings-shallow.e2e.ts`).
  - axe checks in the visual matrix, a 200% zoom test and a reduced-motion test.
  - Status is shown with text labels alongside colour dots (e.g. `ScreeningFocus.svelte:579-592`).
- **Effects are used sparingly** (at most 5 per component) and server data is rarely copied into `$state`. The assistant is the exception.
- **Mobile at 390px works for the core flows.** See `visual-dark-mobile-390x844/workflows.spec.ts/review-title-abstract.png` and `review-full-text.png`: single column, the decision bar inline, and criteria behind a disclosure.

---

## 3. Ranked findings

### F1. Held decision keys record decisions on reports the reviewer never saw
- **Category:** Defect
- **Severity:** P1
- **Confidence:** High for the mechanism; medium for real-world frequency.

**Evidence:**
- `ScreeningFocus.svelte:461-475` and `FullTextScreening.svelte:338-363` act on every `keydown`. Neither handler, nor `screening/shortcuts.ts`, checks `event.repeat` (grep finds no `repeat` anywhere in the screening or full-text features).
- In `decide()` (`ScreeningFocus.svelte:312-369`), the only re-entrancy guard is `decisionMutation.isPending`. It is false during two windows:
  - the `await updateUrl(...)` that runs *before* `mutateAsync`;
  - the `await invalidateScreeningReport()` that runs after it.

**Current behavior:**
- Holding `I` or `E` past the OS auto-repeat delay (about 500 ms; then about 30 repeats/s) records a decision for each successive report as soon as each request settles.
- During the `goto` window, the URL still names the just-removed report. `currentIndex` is then -1, and `current` falls back to `queueItems[0]` (line 124). A second key press in that window therefore decides on the first item in the queue, not the next one.

**Why it matters:**
- Title and abstract screening is the highest-frequency, audit-recorded workflow.
- Undo reverts only the last action (`lastAction`), so a burst of unintended decisions has to be found and reverted one by one from history.

**Root cause:** The decision lock is tied to mutation state instead of a local "decision in flight" flag. Auto-repeat is not filtered. `current` silently falls back to index 0.

**Proposed direction:**
- Ignore `event.repeat` for decision keys. Arrows may keep repeat.
- Hold a synchronous `deciding` flag from the first line of `decide()` until navigation and the mutation settle.
- Capture `reportId` before any await and never fall back to `queueItems[0]` when `urlState.report` is set but missing. Instead, keep the pre-computed `nextItem`.

**Verification:**
- A Playwright test that dispatches `keydown` with `repeat:true` should record exactly one POST.
- A unit test should press I twice within the same tick and expect one decision on the displayed report.

### F2. Document blocks are capped at 100, so evidence overlays and citations silently vanish on longer papers
- **Category:** Defect (frontend and backend)
- **Severity:** P2, possibly P1 for extraction and appraisal evidence
- **Confidence:** High

**Evidence:**
- `FullTextScreening.svelte:176-185`, `AppraisalScreen.svelte:~108` and `ExtractionScreen.svelte:~161` call `createListDocumentBlocks(..., { limit: 100 })` once, with no paging.
- On the backend, `crates/http-api/src/routes/documents.rs:22` sets `MAX_DOCUMENT_LIST_LIMIT = 100`, and `list_document_blocks` has no cursor (the params are only `q` and `limit`).
- `docs/ui-review.md:134` reports v2 parser outputs of 109 and 203 blocks for the two sample PDFs, so typical papers exceed the cap.

**Current behavior:** Blocks after the first 100 get no evidence overlay and cannot be selected or cited in the viewer. Pages later in the PDF look un-parsed.

**Why it matters:**
- Source-grounded appraisal and extraction are a core promise of the product.
- Reviewers will conclude that the evidence does not exist, or that parsing failed.

**Root cause:** The endpoint contract has no pagination for blocks, and the UI never checks whether the result was truncated.

**Proposed direction:**
- Add cursor pagination, or a per-page block fetch (`?page=N`), aligned with the handoff's §4.4 per-page metadata. The viewer should load blocks for visible pages.
- At minimum, show "showing first 100 blocks" when `data.length === limit`.

**Verification:** Fixture a document with 150 blocks spread across pages, and assert that an overlay exists on the last page.

### F3. AI proposal decisions invalidate almost the whole cache but miss the screening queue the user is looking at
- **Category:** Defect / Architecture
- **Severity:** P2
- **Confidence:** High

**Evidence:**
- `orval.config.ts` lists `listTitleAbstractScreeningQueue`, `listFullTextScreeningQueue`, `listProjectDedupeProposals` and `listAiProposals` as invalidation targets for `decideAiProposal`, `generateScreeningSuggestion` and `generateDuplicateSuggestion`.
- Because those targets declare no params, orval emits `query.queryKey[0].startsWith('/api/projects/')` (`api/generated/ai/ai.ts:2319-2333`, and the same at about 2874 and 3216). That predicate matches **every** project-scoped query.
- The title and abstract screen, however, reads the queue through `getScreeningQueue` under the hand key `['screening-queue', …]` (`ScreeningFocus.svelte:81-104`). The AI-proposal invalidation never touches that key.
- `AiProposalReview.svelte:318-342` only refetches its own proposals.

**Current behavior:**
- Requesting or approving an AI suggestion refetches every active project query, including all loaded article pages and the full-text queue.
- Meanwhile the TA queue item keeps the old status and `revision`. The next manual decision on it gets a 409, and the user sees "changed elsewhere", which is a conflict the app caused itself.
- The AI conflict message claims "The current queue was refreshed", which is not true.
- The e2e test (`title-abstract/page.svelte.e2e.ts:503-566`) asserts only that the decision POST was sent, so this slipped through.

**Root cause:** There are two key schemes for the same resource. The TA queue also has two endpoints: `getScreeningQueue` is used, and `listTitleAbstractScreeningQueue` is generated but unused. orval's parameter-less invalidation degrades to a prefix match.

**Proposed direction:**
- Use generated keys everywhere: `getGetScreeningQueueQueryKey(projectId)` as the prefix.
- Point orval invalidations at `getScreeningQueue` with `params: ['projectId']`.
- Delete the unused queue endpoint, or migrate to it.
- Add a lint or test asserting that no generated invalidation emits a bare `'/api/projects/'` predicate.

**Verification:** After "Approve and apply", the e2e test should assert that the queue GET is re-requested and the item leaves "To screen".

### F4. A transient `getProject` error silently moves the user into a different project
- **Category:** Defect
- **Severity:** P2
- **Confidence:** Medium-high

**Evidence:**
- `ProjectWorkspace.svelte:38-43` sets `staleTime: 0`, so a refetch happens on every window focus.
- The effect at `ProjectWorkspace.svelte:107-113` calls `workspace.syncProjectSelection(projects, …, Boolean(projectQuery.error))`.
- `projectToOpen` (`workspace-navigation.ts:9-20`) returns `projects[0]` whenever the route's project "failed".

**Current behavior:**
- A 5xx or network blip on the background refetch sets `error` after one retry, and the user is navigated to the *first* project's overview. They lose their screening position, and nothing explains why.
- A stale or invalid deep link does the same thing with no "project not found" message.

**Why it matters:** The user may continue working in the wrong project, with decisions attributed there.

**Root cause:** Selection fallback treats any error as "project doesn't exist".

**Proposed direction:**
- Fall back only on a 404 from the initial load.
- On other errors, keep the current project and show an inline alert with a retry action.
- On a 404, show an explicit "Project not found" state that links to the project list.

**Verification:** In e2e, return a 503 for the second GET `/projects/{id}` after focus and assert the URL is unchanged.

### F5. Filtering, sorting and counts are computed over loaded pages only (articles, sidebar, full text)
- **Category:** Defect
- **Severity:** P2
- **Confidence:** High

**Evidence:**
- Articles: `ProjectArticlesView.svelte:44-66` filters and sorts `workspace.articles` (50 per page), and `ListProjectReportsParams` has only `cursor` and `limit`. Handoff §4.1 is still open.
- Sidebar badge: `ProjectSidebar.svelte:117-121` shows `workspace.counts.articles`, which is loaded length. It reads "50" for a 5,000-report project.
- Full text, `FullTextScreening.svelte:118-131, 287-294`:
  - The "All", "PDF ready" and "failed" filters and counts are derived from loaded queue pages.
  - "Needs PDF" comes from a different endpoint, capped at `limit:100` with no paging.
  - The 390px baseline `review-full-text.png` shows "All 1 · PDF ready 0 · Needs PDF 0" for a report that has no PDF: the two sources disagree.

**Why it matters:** Reviewers trust counts to decide when a stage is complete, and a search miss reads as "not in corpus".

**Root cause:** List endpoints lack server-side filter, sort and total parameters, and the UI hides that limitation.

**Proposed direction:**
- Implement handoff §4.1 (server `search`, `sort`, `min_internal` and a `total`).
- Add a `document_status` filter and per-status totals to the full-text queue DTO, as the TA queue already does with `progress`.
- Page the missing-PDF list.
- Until then, label counts as "loaded".

**Verification:** Fixture 120 included reports; filter counts must equal server totals.

### F6. The imports history reads a global, unscoped ingestion list and filters it on the client
- **Category:** Defect
- **Severity:** P2
- **Confidence:** High

**Evidence:**
- `ProjectWorkspace.svelte:61-87` uses the `getListIngestionsQueryKey()` cursor list (no project parameter exists), then `.filter(ingestion => ingestion.project_id === selectedProjectId)`.
- The 2 s polling decision scans that global list as well.

**Current behavior:**
- With several projects, a project's runs may sit on later global pages. The imports page then shows no history and drops into composer mode (see the "composer is the page when there are no runs" note in `docs/ui-review.md:116`).
- Polling continues while *another* project's run is active.
- Every project route loads this list.

**Proposed direction:**
- Add `GET /projects/{id}/ingestions`, or a `project_id` filter.
- Move the query into `ProjectIngestionsView` and the overview, out of the layout.

**Verification:** An e2e test with 60 ingestions from another project followed by 1 from the current project should still show the run.

### F7. The xyflow workflow-canvas library ships in the root layout chunk for every route
- **Category:** Behavior-preserving (performance)
- **Severity:** P2
- **Confidence:** High

**Evidence:**
- `routes/+layout.svelte` imports `{ PageFrame } from '@deepref/ui/layout'`.
- `packages/ui/src/patterns/index.ts` re-exports `./workflow-canvas/index.js` and `./chat/index.js`, and `packages/ui/package.json` declares no `sideEffects`.
- In the built entry, node 0 (the root layout) depends on `chunks/DU99vsjb.js` (193 KB raw, `@xyflow/svelte`).
- The root layout totals about 717 KB raw JS. Every project route is 0.6–1.26 MB raw, and each includes xyflow.
- Ten other feature files also import `StatePanel` and `Surface` from the same barrel.

**Why it matters:** The only xyflow consumer is the rarely visited `/automations/[workflowId]`. Slower first load hurts every user.

**Proposed direction:**
- Import `@deepref/ui/page-frame` and `@deepref/ui/state-panel` directly.
- Remove `workflow-canvas` and `chat` from the `layout` barrel, or add `"sideEffects": ["**/*.css"]`.
- Add a bundle-budget check: the root chunk must not contain `xyflow`.

**Verification:** Rebuild and confirm that `DU99vsjb`-equivalent is absent from node 0's `__vite__mapDeps`.

### F8. Polling and refetch cost grows with how far the user has scrolled
- **Category:** Behavior-preserving (performance)
- **Severity:** P3
- **Confidence:** High

**Evidence:**
- Every infinite query refetch re-requests all loaded pages in sequence.
  - The full-text queue does this every 15 s, unconditionally (`FullTextScreening.svelte:106`).
  - Articles do it on every window focus (`staleTime:0`, `ProjectWorkspace.svelte:51-60`), and every 2 s during metric refresh (`ProjectArticlesView.svelte:110-119`).
  - The TA queue invalidates the whole `['screening-queue', projectId]` prefix after *every* decision (`ScreeningFocus.svelte:268-273`), across all status, search and sort variants.
- Full-text `documents` and `document` poll every 5 s even when the PDF is parsed (`:155-175`).
- F3's prefix invalidation multiplies all of this.

**Why it matters:**
- At 1,000 loaded TA items, a decision triggers 40 sequential requests.
- A full-text tab idles at about 0.7 requests/s, before counting the shell's three 10 s pollers.

**Proposed direction:**
- Make `refetchInterval` conditional on non-terminal document state.
- Use `maxPages` on infinite queries.
- After a TA decision, update only the active key and let the next page refill (handoff §4.5).
- Consider one SSE "project events" stream for health, notifications and job state instead of N pollers.

**Verification:** Network log of 50 decisions after loading 10 pages: the request count should be O(decisions), not O(decisions × pages).

### F9. The assistant data layer bypasses svelte-query and the generated client, and hardcodes the actor
- **Category:** Architecture / Defect
- **Severity:** P2
- **Confidence:** High

**Evidence:**
- `features/assistant/chat-api.ts:3-34` redeclares DTOs (`AssistantConversation`, `AssistantMessageRecord`, …), although `listProjectAssistantConversations`, `listProjectAssistantConversationMessages` and `deleteProjectAssistantConversation` are generated and unused.
- `chat-api.ts:38-41` sends `'x-actor-id': 'local-user'` on create, delete and plan confirm/reject. `assistant-stream.ts:307-311` uses `currentReviewerId()` instead, so attribution is inconsistent within one turn.
- `ProjectChatAssistant.svelte:78-125` keeps conversations, turns and budget in `$state`, with `$effect` loading and a manual 10 s `setInterval` (`:256-264`). It never touches `queryClient`.

**Current behavior:** A confirmed assistant plan can apply screening decisions or start imports, but no other cache is invalidated. Overview, PRISMA, queues and activity stay stale until their own staleTime or poll.

**Proposed direction:**
- Move conversations, messages, plans and budget onto generated query hooks.
- Delete `chat-api.ts` types.
- Delete the hardcoded header so `customFetch` supplies the actor.
- On plan completion, invalidate by project prefix, using a single exported `invalidateProjectReviewState(projectId)` helper.

**Verification:** Unit-test that `chat-api` does not override `x-actor-id`. E2E: confirming a plan triggers a refetch of the screening queue.

### F10. Toggling a graph overlay refetches the graph and recomputes the whole layout, so nodes jump and the canvas flashes empty
- **Category:** Defect (UX)
- **Severity:** P3
- **Confidence:** Medium-high

**Evidence:**
- `ProjectGraphView.svelte:45-49`: the overlay `fields` are part of the query params, and therefore the key.
- `graphData` falls back to empty while pending, which runs `graphRenderer.clear()` (`:139-151`).
- On new data, `renderGraph` runs ForceAtlas2 and Noverlap again on the main thread (`project-graph-renderer.ts:851-877, 921-940`). The layout is non-deterministic, so the camera resets and positions change.
- Handoff §4.2 (server-side precomputed coordinates) is still open: `GraphNodeDto` has no x or y.

**Proposed direction:**
- Fetch all overlay fields once, or use `placeholderData: keepPreviousData` and reuse node positions when only attributes change (`refreshAppearance`).
- Precompute layout on the server.

**Verification:** Toggle "screening" and assert that node coordinates are unchanged, via the renderer's test hook.

### F11. Sidebar navigation carries one view's query params into another
- **Category:** Defect
- **Severity:** P3
- **Confidence:** High

**Evidence:**
- `context.svelte.ts:236-241`: `#navigateToView` copies *all* current search params and deletes only `ingestion`.
- Sidebar items with a `view` (overview, protocol, imports, articles, graph, recommendations, prisma) go through it (`ProjectSidebar.svelte:23-35`).

**Current behavior:**
- From `/screening/title-abstract?status=maybe&report=X&mode=focus`, clicking "Articles" opens `/articles?status=maybe&report=X&mode=focus`. `hasInspector` is true, so the article inspector opens on the screening report.
- Graph and article filters also leak into overview and protocol URLs.

**Proposed direction:** Use an allowlist of params per view. The context already knows each view's filters.

**Verification:** E2E: navigate from screening with a report selected to Articles and expect no inspector and a clean URL.

### F12. Crossing a breakpoint swaps whole component trees and loses local state
- **Category:** Behavior
- **Severity:** P3
- **Confidence:** Medium

**Evidence:**
- `ProjectWorkspace.svelte:136-140` renders `ProjectWorkspaceMobile` or `ProjectWorkspaceDesktop`, each rendering `children` in a different subtree, depending on `IsMobile` (768).
- Five feature screens switch layouts at 1024 using two separate templates (`ScreeningFocus.svelte:765-855`, `FullTextScreening`, `AppraisalScreen:59`, `ExtractionScreen:143`, `StudiesScreen:58`).

**Current behavior:** Rotating a tablet (820↔1180) or resizing a window remounts the route. Unsaved inline extraction edits, appraisal form inputs not yet lifted, the assistant composer draft and PDF render state are lost.

**Proposed direction:**
- Keep one tree and switch with CSS and container queries.
- Or lift drafts into a keyed store that survives remount, as the protocol editor already does with localStorage.

**Verification:** A Playwright test that types into an extraction value, sets the viewport from 1100 to 800, and asserts the draft survives.

### F13. Giant components mix data, orchestration and presentation

The 15 largest non-generated, non-test files:

| Lines | File | Responsibilities |
|---|---|---|
| 1787 | `extraction/components/ExtractionScreen.svelte` | 17 queries/mutations, 25 `$state`, 35 functions: study selection, field CRUD, AI generation, proposal review, manual value edit, citation, history |
| 1742 | `automations/AutomationBuilder.svelte` | 1,207-line script: load/adopt, own undo/redo stack, clipboard, notes in localStorage, autosave, validation, publish, test-run polling via `setInterval`, context menus |
| 1271 | `full-text/components/FullTextScreening.svelte` | 15 queries, upload/link/OA fetch, reparse, decisions + 409, keyboard, PDF/evidence navigation |
| 1247 | `protocol/components/ProtocolEditor.svelte` | Form, local draft persistence, autosave, amend/discard, publish, conflict reconcile |
| 990 | `projects/project-graph-renderer.ts` | Sigma lifecycle, layout, hover/drag, palette (acceptable as a non-UI module) |
| 882 | `assistant/components/ProjectChatAssistant.svelte` | Hand data layer (F9), streaming, sessions drawer |
| 872 | `screening/components/ScreeningFocus.svelte` | Queue, optimistic cache surgery, 409, undo, keyboard, two layouts |
| 854 | `appraisal/components/AppraisalScreen.svelte` | Report list, definitions, AI pre-fill, history, 409 |
| 750 | `workflows/domain/validation.ts` | **Dead in production** (see §5) |
| 706 | `automations/model.ts` | Flow↔graph mapping (fine) |
| 695 | `studies/components/StudiesScreen.svelte` | 22 queries/mutations, grouping and classification proposals |
| 678 | `settings/SettingsView.svelte` | OK |
| 675 | `api/assistant-stream.ts` | SSE/run observer (cohesive) |
| 652 | `appraisal/components/AppraisalForm.svelte` | OK-ish |
| 605 | `projects/components/ProjectGraphView.svelte` | OK |

- **Category:** Architecture
- **Severity:** Opportunity
- **Confidence:** High

**Why it matters:**
- Optimistic updates, 409 parsing and undo are implemented twice, differently: `isAuthoritativeState` and `currentStateFromError` in `ScreeningFocus.svelte:242-266` versus `FullTextScreening.svelte:400-407`.
- Defects F1 and F3 hide in this kind of inline orchestration.

**Proposed direction:**
- Extract a `createScreeningSession(projectId, stage)` module (queue key, decide, undo, conflict, keyboard) in `.svelte.ts`, unit-tested without DOM.
- Extract `createDocumentEvidence()`, shared by full text, appraisal and extraction. This also fixes F2 once.
- Split ExtractionScreen into field-schema, study-sheet and proposal subcomponents that own their own mutations.

### F14. Design-system drift lives where the linter cannot see it
- **Category:** Architecture (design system)
- **Severity:** P3
- **Confidence:** High

**Evidence:**
- `<style>` blocks bypass the shadcn lint rules: `ProjectGraphView` (77 lines), `DecisionChoice` (68), `SettingsDialog`, `DecisionBar`, `PrismaFlow`, `FullTextDecisionBar`, `ScreeningFocus` (`kbd`), `ProjectSidebar`.
  - `DecisionChoice.svelte` is a hand-built button with its own hover, focus, disabled and pressed states and raw spacing values. That contradicts AGENTS.md Rule 5 ("declare a new variant in @deepref/ui").
  - `kbd` is styled twice, differently, with no `Kbd` primitive.
- The status-filter chip row is copy-pasted as raw `<button>` markup in `ScreeningFocus.svelte:496-512` and `FullTextScreening.svelte:703-720`, although `toggle-group` exists.
- There are 42 raw `<button>`s in 22 files and 8 native `<select>`s, alongside the `Select` primitive (11 uses).
- The progress bar is hand-rolled (`ScreeningFocus.svelte:741-749`) although `@deepref/ui/progress` exists.
- There are 62 `shadcn/no-restyle` warnings, 13 of them in `ProjectSelector.svelte`.

**Proposed direction:**
- Add `Button` variants `decision-include/exclude/maybe` with `aria-pressed` styling, plus `Kbd` and a `FilterChips` (ToggleGroup `size="xs"`) to `@deepref/ui`.
- Add a stylelint or grep CI check that flags `<style>` in `apps/web` outside an allowlist (graph canvas tokens).

**Verification:** Lint shows 0 `<style>` blocks outside the allowlist, and restyle warnings trend down.

### F15. E2E tests are all mocked with hand-written fixtures, and several journeys are uncovered
- **Category:** Architecture (tests)
- **Severity:** P2
- **Confidence:** High

**Evidence:**
- Every `*.e2e.ts` uses `page.route('http://localhost:4173/api/...')` with inline fixtures. Many declare local types instead of generated DTOs (e.g. `title-abstract/page.svelte.e2e.ts:38-62` `type Report`, `type HistoryItem`).
- No e2e test runs against the real API (only docs record manual runs).
- No e2e exists for:
  - automations list or builder (visual snapshot only);
  - activity feed and undo;
  - the AI-first panel or Conflicts tab (one visual race test);
  - overview pipeline;
  - PMID and file import;
  - project create/rename/delete;
  - full-text upload against real parsing.
- Assertions often stop at "request sent" (F3), so cache coherence between views is untested.
- `routes/demo/playwright/page.svelte.e2e.ts` tests the scaffold demo page.

**Proposed direction:**
- Type every fixture with `#lib/api/generated/models` (`satisfies`).
- Add a small smoke suite against a seeded API (the `scripts/seed-ui.sql` workspace) that covers import → screen → full-text → PRISMA counts. Nightly is enough.
- Add "after mutation X, view Y shows it" assertions for screening, AI proposals and the assistant plan.

### F16. Screen-reader and keyboard gaps on custom surfaces
- **Category:** Accessibility
- **Severity:** P3
- **Confidence:** Medium

**Evidence:**
- `ScreeningFocus.svelte:669` puts `aria-live="polite"` on the whole `<article>`, so every next or decide announces the full title and abstract. The loading skeleton is a live region too (`:661`). A concise status line ("Included. Now 5 of 120: <title>") would serve better.
- The graph canvas (`ProjectGraphView.svelte:506-508`) is a named region, but nodes cannot be reached by keyboard. The Articles table is the implicit alternative, so a "View as list" link inside the region would help.
- On mobile, the shortcut legend sits inside the closed "Eligibility criteria" disclosure. That is acceptable, since mobile users rarely have keyboards.
- The assistant renders a second `<h1>` (`ProjectChatAssistant.svelte:589`) under the top bar's `<h1>` (`TopNavBar.svelte:34`).
- The xyflow builder's keyboard model (`AutomationBuilder.svelte` `onKeydown`) has no a11y test. Narrow screens get a spatial-only editor.

**Verification:** An axe run with `aria-live` review, plus a manual NVDA/VoiceOver pass on TA screening.

### F17. Browser-only state contradicts the persistence invariant
- **Category:** Product / Architecture
- **Severity:** P3
- **Confidence:** High

**Evidence:**
- Canvas sticky notes are saved only to `localStorage` (`AutomationBuilder.svelte:268-303`), while the workflow graph itself persists server-side. The notes look like part of the workflow but do not follow it across devices or users.
- `docs/backend-ui-rebalance-handoff.md` §2.1 says localStorage is for ephemeral UI only.

**Proposed direction:** Persist notes in the workflow definition (layout metadata), or label them "only on this device".

### F18. The PDF viewer renders every page canvas eagerly at device pixel ratio
- **Category:** Behavior-preserving (performance)
- **Severity:** P3
- **Confidence:** High

**Evidence:**
- `PdfViewer.svelte:51-90` loads all `getPage()` calls in sequence before showing anything.
- `PdfPage.svelte:31-50` re-renders every page on every zoom or resize step. Handoff §4.4 is still open.
- The worker asset is 1.26 MB, but it is lazy, which is good.

**Proposed direction:** Render only visible pages plus a buffer (IntersectionObserver), and show the first page as soon as it resolves. The server-tile plan can come later.

---

## 4. UX workflow friction and product simplifications

1. **TA and full-text screening are two different apps.** Filter chips, counts semantics, conflict copy, undo semantics, and "advance after decision" behaviour all differ:
   - TA auto-advances; full text stays on the report.
   - TA conflicts switch the filter to "All"; full text does not.

   One shared screening session (F13) with stage-specific panes would simplify both code and learning.
2. **The AI suggestion sits in a collapsed `<details>` inside the reader.** Requesting it triggers the cache storm in F3. Once approved, the queue does not move. Users must press → to continue, unlike a manual decision.
3. **Counts that disagree** (F5): sidebar "Articles 50", "All 100", and a capped "Needs PDF". Either show server totals or hide the numbers.
4. **The overview, PRISMA and pipeline can lag up to 30 s** after decisions made elsewhere, such as in the assistant or AI-first. Invalidation is per-feature `refetch()` rather than a project-level "review state changed" key. A single `invalidateProjectReviewState(projectId)` called by every review mutation would make the pipeline trustworthy.
5. **The imports page empties when the user has many projects** (F6).
6. **Unknown project links fall into another project** (F4). Show "not found".
7. **Reflowing a tablet resets work** (F12).
8. **The automation canvas on phones** remains a spatial editor (acknowledged in the handoff). Consider making `/automations/[id]` desktop-only, with a read-only summary on mobile. That removes a large mobile test surface.
9. **Product simplification:** `/deduplication` vs `/discovery/duplicates`, and `routes.ts` titles vs per-page `<svelte:head>` titles, duplicate each other with slightly different copy ("Full text screening" vs "Full-text screening"). Keep one route-metadata source.

---

## 5. Deletion candidates

All were verified by grep as having no production importer.

| Candidate | Evidence | Size |
|---|---|---|
| `src/lib/features/workflows/domain/*` (commands, migration, registry, serialization, types, validation) | Imported only by `assistant/workflow-preview.ts`, which is imported only by its test (`workflow-preview.test.ts`). Leftover from the Rete era. | ~2,860 LOC + 788 spec |
| `src/lib/features/assistant/workflow-preview.ts` (+ test) | Same | small |
| `src/routes/demo/**` incl. `page.svelte.e2e.ts` | Scaffold | small |
| `src/lib/vitest-examples/greet*` | Scaffold | small |
| `src/lib/typewriter.svelte.ts`, `src/lib/actions/typewriter.svelte.ts` | No importers | small |
| `src/lib/hooks/use-ramp.svelte.ts`, `use-clipboard.svelte.ts` | Re-exports with no importers | tiny |
| `src/routes/projects/[projectId]/deduplication/+page.svelte` | Unreachable: the sibling `+page.ts` always redirects | small |
| `sass` dependency, the "Rete … SCSS" comment and `vitePreprocess` rationale in `vite.config.ts`, and `rete: false` in `pnpm-workspace.yaml` `allowBuilds` | Rete was removed; there are no `.scss` or `lang="scss"` files | config |
| `pnpm-workspace.yaml` `minimumReleaseAgeExclude` for `@tanstack/query-core@5.101.1` and `svelte-query@6.1.35` | The lockfile resolves 5.104.1 and 6.3.1, so these exclusions are stale | config |
| `@deepref/ui` exports never imported by the app | breadcrumb, chat, close-button, loading-button, metric-tile, page-header, pagination, sidebar, stepper, tags-input, terminal, theme-selector, toggle, window, workflow-canvas (`WorkflowCanvas`; the app uses `BuilderCanvas`). Keep only if Storybook or the design system is a product in itself; otherwise prune and drop them from the `layout` barrel (F7). | ui package |
| Generated-but-unused operations (backend question) | `listAutomationDefinitions`, `listAutomationRuns`, `configureAutomationDefinition`, `runAutomationRecipeOnce`, `triggerAutomationManually`, `getAutomationRun` (legacy automations API superseded by workflows), `getNextScreeningItem`, `listTitleAbstractScreeningQueue`, `resolveProjectRecord`, `getAiProposal`, `getAppraisalDefinition`, `listWorkflowVersions`/`getWorkflowVersion`, and the assistant conversation endpoints (shadowed by `chat-api.ts`) | API surface |
| `features/assistant/chat-api.ts` DTO interfaces | Duplicates generated models (F9) | ~100 LOC |

---

## 6. Open questions

1. Is `getScreeningQueue` or `listTitleAbstractScreeningQueue` the intended TA endpoint? The answer decides the cache-key fix in F3.
2. Is the 100-block cap (F2) intentional for API cost? If so, should the viewer fetch blocks per page, and do appraisal and extraction evidence pickers rely on `q` search to reach the rest?
3. Should `/ingestions` remain global at all, or should it become project-scoped only (F6)?
4. Is `@deepref/ui` meant to be published or reused outside this app? That decides whether to prune the unused exports and stories or keep them.
5. Is tablet portrait (768–1023) a supported screening form factor? If yes, F12 matters more.
6. Is there appetite for a single project event stream (SSE) to replace the five or more pollers (F8)?
7. Should the `@tanstack/svelte-store` `Object.is` patch be upstreamed? It exists only for `@tanstack/svelte-form`, which only `ProtocolEditor.svelte` uses.
8. Bundle numbers come from the 2026-10-08 build. They should be rerun once `/tmp` space allows: `pnpm --filter @deepref/web build`.
