# UI review

This pass focuses on a quiet, readable research workspace with task controls close to the evidence. It retains the existing Svelte, shadcn-svelte, IBM Plex Sans, and teal identity.

## References

- [Dribbble research dashboards](https://dribbble.com/tags/research-dashboard): inspected the Movade research workspace and Academic SaaS concepts for navigation, content grouping, and placement of search.
- [Awwwards minimal gallery](https://www.awwwards.com/websites/minimal/): explored restrained typography and spacing references. Product tasks remain the priority over decorative motion.

## Page decisions

| Page               | Change                                                                                                               |
| ------------------ | -------------------------------------------------------------------------------------------------------------------- |
| Overview           | Direct entry to screening, with shortcuts to imports, connections, and reporting. Metrics are secondary.             |
| Protocol           | Smaller heading and quieter context labels give the protocol form more space.                                        |
| Articles           | Removed duplicate metric tiles; search and the article table appear earlier. Inspector appears only after selection. |
| Imports            | Removed redundant counters; plain instructions explain DOI entry. Inspector appears only for a selected run.         |
| Duplicates         | Clear explanation of what to compare and how to start checking.                                                      |
| Title and abstract | Compact progress row, wider reading area, optional AI assistance.                                                    |
| Full text          | More compact header leaves more room for the source and decision controls.                                           |
| Studies            | Existing groups take priority; new-group form opens on request.                                                      |
| Appraisal          | Clearer instructions for selecting an article and assessment framework.                                              |
| Extraction         | Plain explanations of study selection, fields, proposals, and saved values.                                          |
| PRISMA             | Diagram and exports precede exhaustive counts, which remain available in a disclosure.                               |
| Graph              | Graph comes before optional overlay, legend, and update details.                                                     |
| Recommendations    | Wrapped article titles, fewer nested scroll areas, update details available on request.                              |
| Automations        | Clearer maintenance setup and run language.                                                                          |
| Assistant          | Describes useful actions and review responsibility instead of internal tool contracts.                               |
| Settings           | Added a return link to the workspace.                                                                                |

Shared changes include consistent navigation links and active states, quieter light and dark palettes, and reduced decorative borders and metric colors. Reduced-motion and keyboard focus support are retained.

## Local sample workspace

`scripts/seed-ui.sql` creates a fictional ambient documentation review with 12 articles, 22 citation links, a published PICO protocol, mixed screening states, two study groups, and extraction fields. The data is explicitly marked as sample evidence. It is idempotent and retains existing decisions.

The review instance uses a dedicated `deepref-human-touch-postgres` container, database `deepref_ui_review`, PostgreSQL port 5443, API port 8083, and web port 5183. This avoids other worktrees' development services.

```bash
# Restart the existing dedicated database:
docker start deepref-human-touch-postgres

# Seed after migrations:
docker exec -i deepref-human-touch-postgres psql -v ON_ERROR_STOP=1 -U postgres -d deepref_ui_review < scripts/seed-ui.sql

# API (run from repository root):
APP_ENV=local DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5443/deepref_ui_review API_BIND_ADDR=127.0.0.1:8083 DOCUMENT_STORAGE_BACKEND=local DOCUMENT_STORAGE_ROOT=/tmp/deepref-ui-documents target/debug/deepref-server serve

# Worker (another terminal, for background tasks):
APP_ENV=local DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5443/deepref_ui_review DOCUMENT_STORAGE_BACKEND=local DOCUMENT_STORAGE_ROOT=/tmp/deepref-ui-documents target/debug/deepref-server worker

# Web (another terminal):
API_PROXY_TARGET=http://127.0.0.1:8083 mise exec -- pnpm dev:web --host 127.0.0.1 --port 5183
```

Open `/projects/00000000-0000-4000-8000-000000000201/overview` on the web server.

## Earlier verification

The full browser suite passes 65 tests, and the unit suite passes 122 tests. Desktop and mobile browser inspection covers the workflow pages, light and dark themes, and article selection. Automated accessibility checks report no serious or critical violations for the checked overview, mobile article list, and open mobile inspector. Phone inspectors use the full screen width, with keyboard-accessible scrolling.

## Scroll and section-layout follow-up

The desktop workspace now owns a bounded scrolling region for route content, including screening and appraisal pages that previously grew inside clipped panes. Long form pages flow naturally inside it. Mobile uses the same viewport.

Protocol uses Research question, Framework, and Eligibility criteria tabs; Extraction uses Fields, Review proposals, and Accepted values; Automations uses Setup, Run now, and Run history; Recommendations separates its three reading groups into tabs; Appraisal separates the assessment from optional AI suggestions. Form state remains in the parent screen while switching tabs. Major task containers are semantic sections rather than decorated cards. Shared surfaces and metrics are flat, and redundant header breadcrumbs and study summary tiles are removed.

Browser checks at a 600px desktop height and a 390px-wide mobile viewport verified that all overflowing regions on the 15 workflow routes could reach their bottom. The protocol browser test also exercises reaching Save draft at a 500px height.

## Destination workspace restructure — 2026-09-30

The existing shell, sidebar destinations, and route hierarchy are unchanged. The redesign is inside each destination:

| Destination      | Workspace decision                                                                                                                                                                                            |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Overview         | Actionable evidence list replaces the metric-card dashboard; recent imports are a disclosure rather than a competing primary view.                                                                            |
| Protocol         | Full-width authoring tabs with compact lifecycle actions; no permanent, mostly empty lifecycle column.                                                                                                        |
| Imports          | DOI-first submission; citation depth is secondary configuration, and the form closes out of the way when run history exists. Selected runs expose progress and articles first, polling diagnostics on demand. |
| Articles         | Bounded, scrolling desktop collection; direct narrow-screen selection; no bulk-selection controls without a bulk action. Search and minimum-citation filters share URL state across screen sizes.             |
| Deduplication    | Collection plus comparison, with readable identifiers first and complete identifier provenance behind disclosure.                                                                                             |
| Title & abstract | Resizable queue/reader workspace; narrow screens give the active record the reading area rather than reserving an empty queue column.                                                                         |
| Full text        | Document-first, resizable 68/32 document/review split; natural-height stacked regions on narrow screens.                                                                                                      |
| Studies          | Resizable collection/details context; study creation opens on demand, with classification, membership, and history retained.                                                                                  |
| Appraisal        | Evidence beside assessment; substantial assessment, AI suggestion, and history tasks use local views. Missing reports, unavailable parsed evidence, and query failures have distinct states.                  |
| Extraction       | Structured fields, proposals, and accepted values remain focused local views, with contextual source evidence and progressive field creation.                                                                 |
| PRISMA           | Canonical flow occupies the main pane; Counts & sources and Export are separate local views. Narrow diagrams retain readable nodes and keyboard-accessible horizontal scrolling.                              |
| Graph            | Continuous canvas; search and reset overlay the graph, with layers, legend, and projection details in contextual controls.                                                                                    |
| Recommendations  | Searchable reading lists; selecting a recommendation opens details without leaving discovery or losing the current tab/search.                                                                                |
| Automations      | Recipes and canvas editing are focused local tasks. Node properties, palette, and run monitoring are contextual; browser-local graphs are not presented as executable backend workflows.                      |
| Assistant        | Conversation owns the pane; saved sessions use a dismissible, focus-contained drawer, not a permanent sidebar.                                                                                                |
| Settings         | Ingestion and Appearance are the useful categories; provider limits are advanced, autosaved configuration. Search reveals matching advanced fields.                                                           |

Interaction fixes include seed-only imports preserving citation depth `0`, stable collection mounting while an inspector opens, responsive article filters, recipe Run buttons opening their launch dialog, polling preserving an expanded activity list, and assistant-session focus containment/restoration.

Real-browser inspection covered 1440×1000 and 390×844 layouts, populated and empty collections, selection, local view changes, resizable boundaries, forms, and contextual overlays. Mutations used disposable projects: protocol amendment/publication, screening and undo, duplicate acceptance, study classification/membership, extraction schema creation, import cancellation, recipe submission, and persisted assistant sessions. Settings validation/autosave were exercised and the original contact setting restored.

Runtime limits: the worker was degraded, imports and maintenance executions remained queued, and usable parsed PDF blocks were unavailable. Source-backed full-text completion, appraisal completion, and extraction-proposal acceptance are therefore not claimed verified. The actual assistant endpoint returned a greeting without tool/citation events; no fake evidence or mock backend fallback was added to the application.

Final integration exercised overview-to-article and overview-to-import deep links against the API, graph-filter reset, and dismissal of the narrow-screen article inspector before using canvas tools. The graph legend uses the full contextual-overlay width, and the canvas is a named region for assistive technology. Browser contracts now enter the relevant local PRISMA view and graph overlay instead of expecting secondary controls to remain permanently visible.

Workspace verification passed: `pnpm check` (one `ScreeningTable.svelte` initial-value warning), `pnpm test` (210 tests), `pnpm lint` (zero errors, 90 warnings), production build, and all 78 browser workflow tests. The four-viewport light/dark visual and accessibility matrix passed 114 tests; two desktop-table checks are intentionally skipped on mobile. Visual baselines were refreshed and inspected.

## Second workspace pass — 2026-10-06

The shell, sidebar destinations, and routes are unchanged. This pass targeted the work inside the destinations that the first pass had restyled but not restructured.

| Destination      | Change                                                                                                                                                                                                                                                                                                                               |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Overview         | Review pipeline derived from the PRISMA projection and protocol status (`review-progress.ts`). The page names the next step itself, for example 1 maybe or 8 missing PDFs, instead of always offering "Continue screening". The corpus list and imports form a side column.                                                          |
| Title & abstract | Queue, reader, and criteria are three resizable panes. Status filters with counts replace the hidden "Queue filters" disclosure. Eligibility criteria and history sit beside the abstract. The decision bar is a footer whose pressed button is the recorded decision. Completion states route to maybes or full text and keep Undo. |
| Full text        | A queue pane shows PDF state and decision per report. Attaching a PDF happens in the document surface; the filename is no longer asked for. Exclude opens the reasons, and choosing one commits it. The I/E/M/U, arrow, and 1–9 shortcuts now exist; before, they were only advertised.                                              |
| Studies          | The first study opens automatically. Creation is inline. Rename and classification live in the study header. Adding a report that belongs to another study states the move before it happens. The list no longer shows a misleading "0 reports" (see the known gap below).                                                           |
| Appraisal        | A list of full-text-included reports (all reports one click away) replaces the all-reports dropdown, and the first is opened. The AI pre-fill is a strip above the form it fills, not a separate tab. The framework is a compact select, and history is a header toggle.                                                             |
| Extraction       | Studies list, a single study sheet (each field with its accepted value or "—", with a pending proposal above it), and source evidence. Field management is a secondary view; the key and version are derived from the label under a disclosure.                                                                                      |
| Protocol         | A published protocol reads as a document with one Amend action. Editing has one sticky status/action bar, and an amendment can now be discarded without reloading.                                                                                                                                                                   |
| Imports, Dedup   | The composer is the page when there are no runs, and the DOI count drives the button label. Duplicate counts and status chips are collapsed into one heading.                                                                                                                                                                        |
| Articles, Recs   | Count and metrics status are on one line. Single-page pagination is hidden. Recommendation rows are one line, and selection uses the shared accent treatment.                                                                                                                                                                        |
| Automations      | The empty state leads to recipes. Empty activity and a duplicate page heading are removed.                                                                                                                                                                                                                                           |
| Assistant        | Starter questions send directly from an empty state aligned with the composer.                                                                                                                                                                                                                                                       |

Shared: `DecisionChoice` replaces Button restyling for screening decisions. `CriteriaPanel` and `ScreeningHistory` are plain pane content. Redundant in-page `h1`s that duplicated the top bar title are removed.

Known gap: `GET /projects/{id}/studies` returns `reports: []` for every study, while the study detail endpoint returns members. The list therefore cannot show member counts until the list projection includes them.

Verification: `pnpm check` (0 errors), `pnpm test` (216), `pnpm lint` (0 errors; warnings 90 → 70), 78/78 e2e, and the 114-test visual/axe matrix with refreshed baselines. Real-browser exercise against the seeded workspace covered resolving a maybe and undoing it, the full-text empty filters, extraction field creation (derived key), protocol amend and discard, and the overview at 1440 and 390 px.

## Fixes from the end-to-end walkthrough — 2026-10-06

The walkthrough report (`docs/ux-walkthrough/relatorio-2026-10-06.html`) listed 28 findings. All but F01 were fixed (F01 is the Crossref email, which will come from user accounts). Eleven parallel agents did the work, each owning separate files. The changes were then integrated, reviewed, and re-run in a real browser against the seeded test project.

| Area                         | Change                                                                                                                                                                                                                                                                                                                                                                                                                          |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Full text                    | Failed PDFs show a plain-language reason and offer **Retry processing**. Re-uploading the same file re-queues it instead of returning 409 (`POST …/documents/{id}/reparse`). The viewer fits the pane width and has zoom controls. Block outlines appear only on hover, focus, or selection. Exclusion reasons are listed by relevance, confirmations no longer shift the layout, and decision history reads as plain language. |
| PDF parsing                  | Blocks are now paragraphs, headings, and table rows instead of line fragments. The 7-page fitbit PDF went from 501 to 109 blocks and the IDEA PDF from 1599 to 203. Repeated running headers and page numbers are dropped. Parser version is `deepref-pdfium-0.9-v2`.                                                                                                                                                           |
| Worker stability             | One PDFium instance per process (`PdfiumParser::shared_from_env`), with parses serialized. Per-job instances let one drop tear down the library mid-parse; with the slower v2 parser this aborted the worker with heap corruption. A concurrency test now covers it.                                                                                                                                                            |
| Health                       | The worker is "degraded" only for recent failures or a stale queue, not for any historical dead job. The global toast is replaced by a quiet top-bar indicator that never covers actions.                                                                                                                                                                                                                                       |
| Shell                        | One project selector (sidebar on desktop, compact on mobile). The duplicate "Agent" link is removed and the mobile header is a single row. Notification states are consistent. `PaginationLoadMore` shows only the action. Toasts sit at the top, away from bottom decision bars.                                                                                                                                               |
| Extraction                   | Reviewers can record, overwrite, and clear values without AI (`PUT/DELETE …/extraction/values/{field}`). Values edit inline, can cite a source passage, and keep their history (migration `0025`). The AI-not-configured note appears up front, and provenance shows page and title instead of IDs. The add-field form resets and fields keep creation order.                                                                   |
| Protocol                     | Drafts autosave. Amendments stay on the device until an explicit save, so they can still be discarded. Validation runs on attempt, with a publish checklist. Labels are consistent (PICO, "Title & abstract", "Dimension"). Version history with field-level diffs (`GET …/review/protocol/versions`). Less decoration, and the page scrolls to top after publishing.                                                           |
| AI                           | `GET /ai/status`. Suggestion cards collapse to one honest line when AI isn't set up. The assistant no longer answers with a canned greeting and explains that it needs a provider. The "Usage" disclosure and duplicate "New chat" are removed. The assistant tool envelope now rejects unknown top-level fields; this was a pre-existing fail-open caught by the integration run.                                              |
| Automations                  | Editing is a short form (name, when, run, active) with that automation's runs. The non-executing node canvas is removed. Running a recipe once no longer creates an automation. Run results appear inline, with per-row Run now and pause, human trigger labels, and no internal IDs. The recipe label is truthful: "Refresh project metrics".                                                                                  |
| Collect                      | The depth control explains its consequences, warns at depth 2 or more, and remembers the last choice. The import inspector shows titles. Deduplication reports what it checked. Dialog copy is trimmed.                                                                                                                                                                                                                         |
| Overview, studies, appraisal | The pipeline shows on empty projects (protocol first) and reflects appraisal and extraction progress. A completed appraisal shows as a read-only summary. Study report candidates are limited to included reports, and internal revisions are hidden.                                                                                                                                                                           |
| Analysis                     | Standard PRISMA 2020 diagram listing only reasons that occurred. The graph fits its content, keeps clear of the floating toolbar, and places isolated nodes below the cluster (sigma's y axis points up). Recommendations exclude screened-out articles and say what they rank.                                                                                                                                                 |
| Import metadata              | Missing Crossref abstracts are filled from Europe PMC, with OpenAlex as fallback. Cleaned to plain text, bounded and best-effort. Opt out with `DEEPREF_ABSTRACT_ENRICHMENT=off`.                                                                                                                                                                                                                                               |

Verification: `cargo fmt`/`clippy` clean, 360 Rust tests passed (DB tests on a throwaway database), `pnpm check` 0 errors, `pnpm test` 223 passed, `pnpm lint` 0 errors (70 warnings), 83/83 e2e, and the visual/axe matrix 102 passed + 2 intentional skips, with all baselines regenerated. `workflow-viewport.spec.ts` was removed with the canvas it tested. Browser re-run on the test project confirmed:

- the Europe PMC abstract on a new import;
- re-parsed PDFs, now 109/203 active v2 blocks;
- a manual extraction value with a cited passage;
- protocol autosave surviving a reload;
- the pipeline, PRISMA, graph, assistant, and automation one-off runs.

Known gaps:

- Parsed front matter still contains joined fragments, such as "andBrittany".
- The assistant has no language-model reply path, only tool commands.
- Review-type recipes run only from the assistant.
- `reparse` does not record the actor.
- The studies list still returns no members.
- The "Get an AI suggestion" disclosure is still shown when AI is unavailable, though it now explains why.
