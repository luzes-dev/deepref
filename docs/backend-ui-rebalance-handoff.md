# Handoff: Frontend-to-Backend Architectural Rebalance

> **Authoritative Handoff Document**  
> **Target Audience:** Autonomous Coding Agent / Engineer  
> **Scope:** Architecture, database schema, Rust crates, HTTP API endpoints, Svelte 5 frontend components, and phased implementation steps for migrating heavy client-side computations to authoritative backend services.

---

## 1. Executive Summary & Objective

The DeepRef monorepo pairs a high-performance Rust backend (`axum`, `sqlx`, `pgvector`, `petgraph`, `pdfium-render`) with a SvelteKit single-page application (`apps/web` with `@deepref/ui`). 

An architectural audit of `apps/web` reveals that the client currently acts as an overloaded **"thick client"**, executing algorithmic workloads, maintaining disconnected local storage domain models, performing client-only filtering over loaded pagination slices, and managing fragile multi-page cache surgery.

This handoff specifies the migration of six subsystems from the browser to authoritative backend crates and services:

1. **Articles Catalog (FTS, Filter, Sort)**: Migrate client-side array filtering/sorting over loaded 50-item chunks into indexed PostgreSQL queries with full-text search and cursor pagination.
2. **Citation Graph Layout**: Precompute deterministic 2D coordinates `(x, y)` in `crates/graph` during worker projection passes, turning the frontend into an instant (<16ms) WebGL renderer.
3. **Workflow Hub Persistence & Worker Execution**: Move Rete workflow DAG graphs from browser `localStorage` into PostgreSQL tables, enforce DAG validation in `crates/domain`, and enable `services/worker` execution.
4. **Full-Text Document Page Streaming**: Leverage native C++ `pdfium-render` in `crates/documents` to stream on-demand page WebP/PNG tiles with pre-aligned bounding box coordinates, eliminating the heavy `pdfjs-dist` client bundle and DOM canvas memory bloat.
5. **Screening Queue Projection**: Rebalance screening state synchronization to rely on server-projected queue windows and lightweight invalidation rather than manual multi-page cache surgery in TanStack Query.
6. **Publication-Ready PRISMA Export**: Add server-side headless rasterization (PNG/PDF) to `crates/http-api/src/routes/exports.rs` for deterministic publication-grade flowcharts.

*(Note: Assistant chatbot orchestration and LLM conversational streaming are handled separately under [`docs/assistant-and-workflows-handoff.md`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/docs/assistant-and-workflows-handoff.md).)*

---

## 2. Architectural Invariants

Every change must uphold the following system invariants:

1. **PostgreSQL as Single Source of Truth**: All relational state, graph metrics, projections, and workflow definitions belong in PostgreSQL. Browser `localStorage` must only store ephemeral client UI state (e.g. collapsed sidebar preferences).
2. **Authoritative Server Projections**: Derived views (graph topology, recommendation clusters, PRISMA flow tallies, screening counts) are computed asynchronously by worker jobs using durable PostgreSQL leases (`FOR UPDATE SKIP LOCKED`).
3. **Deterministic & Reproducible**: Given identical project reports and review decisions, graph layout coordinates and export artifacts must be bit-for-bit or coordinate-for-coordinate deterministic.
4. **Thin & Responsive Client**: The browser UI must prioritize 60 FPS interactions, virtualized DOM nodes, and responsive gestures. Heavy computational loops (graph physics, PDF page parsing, multi-pass filtering) belong on the backend.
5. **Typed OpenAPI Boundary**: All new backend functionality must be exposed via `utoipa` annotations on `axum` handlers, updating `docs/openapi.json` and generating type-safe client SDK hooks via `pnpm run generate:api`.

---

## 3. Existing Codebase Anchors

| Subsystem | Frontend Location | Backend Location |
| :--- | :--- | :--- |
| **Articles Catalog** | [`ProjectArticlesView.svelte`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/projects/components/ProjectArticlesView.svelte)<br>[`ProjectWorkspace.svelte`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/projects/components/ProjectWorkspace.svelte)<br>[`context.svelte.ts`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/projects/context.svelte.ts) | [`crates/http-api/src/routes/articles.rs`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/http-api/src/routes/articles.rs)<br>[`crates/postgres/src/`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/postgres/src/) |
| **Citation Graph** | [`project-graph-renderer.ts`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/projects/project-graph-renderer.ts)<br>[`ProjectGraphView.svelte`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/projects/components/ProjectGraphView.svelte) | [`crates/graph/src/`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/graph/src/)<br>[`crates/http-api/src/routes/articles.rs`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/http-api/src/routes/articles.rs)<br>`services/worker/src/processor.rs` |
| **Workflow Hub** | [`apps/web/src/lib/features/workflows/`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/workflows/)<br>[`AutomationCenter.svelte`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/automations/components/AutomationCenter.svelte)<br>[`AutomationEditor.svelte`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/automations/components/AutomationEditor.svelte) | [`crates/application/src/automations.rs`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/application/src/automations.rs)<br>[`crates/postgres/src/automations.rs`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/postgres/src/automations.rs)<br>[`crates/review/src/definition.rs`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/review/src/definition.rs)<br>`services/worker/src/processor.rs` |
| **Document Full-Text** | [`PdfViewer.svelte`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/full-text/components/PdfViewer.svelte)<br>[`PdfPage.svelte`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/full-text/components/PdfPage.svelte) | [`crates/documents/src/parser.rs`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/documents/src/parser.rs)<br>[`crates/http-api/src/routes/documents.rs`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/http-api/src/routes/documents.rs) |
| **Screening Queue** | [`optimistic.ts`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/screening/optimistic.ts)<br>[`ScreeningFocus.svelte`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/screening/components/ScreeningFocus.svelte) | [`crates/http-api/src/routes/review.rs`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/http-api/src/routes/review.rs)<br>[`crates/application/src/`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/application/src/) |
| **PRISMA Exports** | [`png.ts`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/prisma/png.ts) | [`crates/application/src/prisma.rs`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/application/src/prisma.rs)<br>[`crates/http-api/src/routes/exports.rs`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/http-api/src/routes/exports.rs) |

---

## 4. Subsystem Specifications

### 4.1. Articles Catalog: Server-Side Query, Filter, & Keyset Sort

#### The Defect
In [`ProjectArticlesView.svelte:L28-L52`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/projects/components/ProjectArticlesView.svelte#L28-L52), `filtered` runs in-memory JavaScript `.filter()` and `.toSorted()` over `workspace.articles`. Because `workspace.articles` is populated via infinite scrolling in 50-item batches, searching or sorting only evaluates loaded items. In a project with 5,000 articles, un-fetched articles matching the search term or sort criteria are completely invisible.

#### Target Implementation
1. **Backend Endpoint Updates ([`crates/http-api/src/routes/articles.rs`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/http-api/src/routes/articles.rs))**:
   - Enhance `PaginationParams` or define `ListReportsQuery`:
     ```rust
     #[derive(Debug, Deserialize, IntoParams)]
     pub(crate) struct ListReportsQuery {
         pub cursor: Option<String>,
         pub limit: Option<i64>,
         pub search: Option<String>,
         pub sort: Option<ReportSortField>, // rank | internal | total | year | title
         pub min_internal: Option<i32>,
         pub screening_status: Option<String>,
     }
     ```
   - In the SQL query builder:
     - Apply search condition when `search` is present: use PostgreSQL `to_tsvector('english', r.title || ' ' || COALESCE(r.abstract_text, '')) @@ plainto_tsquery('english', $search)` or trigram matching `r.title ILIKE ('%' || $search || '%') OR doi.value ILIKE ('%' || $search || '%')`.
     - Filter `pr.internal_citations >= $min_internal` when specified.
     - Support keyset pagination tuple tailored to the active `sort` direction (e.g. `(pr.rank_score, pr.report_id) < ($1, $2)` vs `(r.publication_year, pr.report_id) < ($1, $2)`).
2. **Frontend Wiring**:
   - Update `articlesQuery` in [`ProjectWorkspace.svelte`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/projects/components/ProjectWorkspace.svelte) to pass `workspace.articleFilters.filter`, `workspace.articleFilters.sort`, and `workspace.articleFilters.minInternal` directly into the query key and API request.
   - Remove client-side `.filter()` and `.toSorted()` in `ProjectArticlesView.svelte`, rendering `workspace.articles` directly.
3. **Completion Criteria**:
   - `GET /projects/{id}/reports?search=cardio&sort=year` executes on PostgreSQL and returns true global project results.
   - Keyset pagination correctly traverses subsequent pages under any sort field.
   - Searching for a keyword present only on record #450 returns that record immediately on page 1 without pre-scrolling.

---

### 4.2. Citation Graph: Precomputed Layout Coordinates in Worker

#### The Defect
In [`project-graph-renderer.ts:L820-L845`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/projects/project-graph-renderer.ts#L820-L845), the browser dynamically loads `graphology`, `sigma`, `graphology-layout-forceatlas2`, and `graphology-layout-noverlap`. Every time the user navigates to the graph, the browser runs 120–300 iterations of Barnes-Hut and 150–500 iterations of Noverlap on the main JavaScript thread, locking the UI and yielding non-deterministic coordinates across sessions.

#### Target Implementation
1. **Schema & Model Updates**:
   - In `crates/postgres/migrations/`, add `graph_layout_x DOUBLE PRECISION` and `graph_layout_y DOUBLE PRECISION` to `project_reports` (or a dedicated `project_graph_layouts` table keyed by `(project_id, report_id, projection_revision)`).
   - In [`crates/graph/src/model.rs`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/graph/src/model.rs), update `GraphNode` to include:
     ```rust
     pub x: Option<f64>,
     pub y: Option<f64>,
     ```
   - In [`crates/http-api/src/routes/articles.rs`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/http-api/src/routes/articles.rs), update `GraphNodeDto` to serialize `x` and `y`.
2. **Worker Graph Layout Pass**:
   - In `services/worker/src/processor.rs` (during project graph projection / metrics recomputation):
     - Load `StableDiGraph` from `crates/graph`.
     - Execute a deterministic 2D layout in Rust. You can port/use an established force-directed layout (e.g. Fruchterman-Reingold or ForceAtlas2 in Rust with parallel Rayon step computations) with a fixed seed.
     - Persist `(graph_layout_x, graph_layout_y)` coordinates into PostgreSQL upon projection completion.
3. **Frontend Optimization**:
   - In `project-graph-renderer.ts`, inspect incoming nodes. If `node.x` and `node.y` are provided by the backend DTO, skip `forceAtlas2.default.assign` and `noverlap.default.assign` entirely.
   - Instantly populate Graphology node positions: `nextGraph.addNode(node.id, { x: node.x, y: node.y, size: node.size, ... })`.
   - Keep client-side ForceAtlas2 only as an explicit manual "Re-simulate in Browser" user button.
4. **Completion Criteria**:
   - Loading `/projects/{id}/graph` displays all nodes within 16ms of data arrival with zero main-thread freezing.
   - Node positions are visually identical across different browsers, sessions, and screen sizes.

---

### 4.3. Workflow Hub: Authoritative PostgreSQL Persistence & Worker Execution

#### The Defect
In [`AutomationCenter.svelte:L310-L335`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/automations/components/AutomationCenter.svelte#L310-L335) and [`graph.ts`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/automations/graph.ts), visual workflow graphs authored in Rete are saved **only to `localStorage`** (`deepref:automation-graph`). The backend API only stores `{ name, trigger, status }` for a hardcoded recipe (`BuiltInAutomationRecipe::ProjectMaintenanceV1`). Custom graphs cannot be executed by the worker and disappear if cache is cleared.

#### Target Implementation
1. **PostgreSQL Workflow Storage**:
   - Add migration in `crates/postgres/migrations/` creating `automation_workflows`:
     ```sql
     CREATE TABLE automation_workflows (
         id UUID PRIMARY KEY,
         project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
         definition_id UUID NOT NULL,
         schema_version INT NOT NULL DEFAULT 1,
         name VARCHAR(200) NOT NULL,
         definition JSONB NOT NULL,
         created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
         updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
     );
     CREATE UNIQUE INDEX idx_automation_workflows_project_def ON automation_workflows(project_id, definition_id);
     ```
2. **Backend Domain & Schema Validation**:
   - In [`crates/application/src/automations.rs`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/application/src/automations.rs), define `WorkflowDocument` matching the versioned JSON shape from [`apps/web/src/lib/features/workflows/domain/types.ts`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/workflows/domain/types.ts).
   - Authoritatively validate:
     - Schema version compatibility.
     - Known node kinds and valid node parameters.
     - Connection legality (source port output type matches target port input type).
     - Acyclic graph constraint (topological sort via `petgraph`).
3. **HTTP API Routes**:
   - Add routes in [`crates/http-api/src/routes/automations.rs`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/http-api/src/routes/automations.rs):
     - `GET /projects/{project_id}/automations/{definition_id}/workflow` -> `WorkflowDto`
     - `PUT /projects/{project_id}/automations/{definition_id}/workflow` -> saves and validates `WorkflowDto`.
4. **Worker DAG Interpretation**:
   - In `services/worker/src/processor.rs`, allow automation runs to reference custom `automation_workflows`.
   - Traverse the DAG nodes in topological order, dispatching steps via existing worker job infrastructure.
5. **Frontend Client Integration**:
   - Update `AutomationCenter.svelte` and `AutomationEditor.svelte` to fetch and persist workflows via the new API endpoint instead of `saveAutomationGraphDraft` to `localStorage`.
6. **Completion Criteria**:
   - Workflows created in the visual editor persist in PostgreSQL and remain accessible across private windows or different client devices.
   - Submitting a malformed or cyclic workflow returns `400 Bad Request` with an explicit JSON validation error.
   - The worker executes a custom workflow run step-by-step with status visible in `AutomationRunHistory.svelte`.

---

### 4.4. Full-Text Document Streaming: Native Pdfium Page Tiles & Bounding Boxes

#### The Defect
In [`PdfViewer.svelte:L50-L72`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/full-text/components/PdfViewer.svelte#L50-L72), the frontend bundles `pdfjs-dist` (~2.5MB), downloads the entire PDF file (up to 50MB), and eagerly instantiates an HTML `<canvas>` for all pages at once. For a 40-page paper, this consumes 300MB–500MB of browser RAM. Concurrently, the backend already contains Google's C++ Pdfium library via `pdfium-render` ([`crates/documents/src/parser.rs`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/documents/src/parser.rs)).

#### Target Implementation
1. **Server-Side Page Rasterization Endpoint**:
   - In [`crates/http-api/src/routes/documents.rs`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/http-api/src/routes/documents.rs), add:
     ```rust
     GET /projects/{project_id}/documents/{document_id}/pages/{page_number}/tile?scale=2.0
     ```
   - Handler uses `pdfium-render` (`PdfiumParser` or direct `PdfiumPage::render`) to render the requested page into a WebP or PNG byte stream with HTTP caching headers (`Cache-Control: public, max-age=31536000, immutable`).
2. **Metadata & Bounding Boxes**:
   - Provide page dimensions and pre-calculated text/evidence bounding boxes:
     ```rust
     GET /projects/{project_id}/documents/{document_id}/pages
     ```
     Returning `[{ page_number: 1, width: 612, height: 792, blocks: [...] }]`.
3. **Frontend Virtualized Viewer**:
   - Refactor `PdfViewer.svelte` to use virtual scrolling (rendering only visible pages plus a 2-page buffer).
   - Replace `<canvas>` in `PdfPage.svelte` with standard `<img>` tags pointing to the server tile endpoint, with an SVG overlay for evidence bounding boxes.
   - Remove `pdfjs-dist` dependency from `apps/web/package.json`.
4. **Completion Criteria**:
   - Opening a 50-page PDF downloads only the active page tile and metadata, lowering initial memory consumption by >80%.
   - `apps/web` bundle size drops by ~2.5MB with the removal of `pdfjs-dist`.
   - Evidence highlights line up with pixel-perfect accuracy on top of the rendered image tiles.

---

### 4.5. Screening Queue: Server-Projected Window & Simplified Client Cache

#### The Defect
In [`optimistic.ts:L23-L60`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/screening/optimistic.ts#L23-L60), the frontend performs extensive manual mutation across nested pages in the TanStack Query cache (`findQueueLocation`, `progressAfter`, `updatePageItems`). When rapid keyboard triage occurs or when the server raises `409 screening_revision_conflict`, the client cache desynchronizes, showing duplicate cards or incorrect counter totals.

#### Target Implementation
1. **Server Queue Window Projection**:
   - Verify that [`crates/http-api/src/routes/review.rs`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/http-api/src/routes/review.rs) (`getScreeningQueue`) returns the current queue slice with authoritative progress counters:
     ```rust
     pub struct ScreeningQueueResponseDto {
         pub items: Vec<ScreeningQueueItemDto>,
         pub total_pending: i64,
         pub total_screened: i64,
         pub next_cursor: Option<String>,
     }
     ```
2. **Simplified Optimistic Model**:
   - Remove multi-page array shifting in `optimistic.ts`.
   - Instead, maintain a simple local `Set<ReportId>` for immediately acknowledged triage decisions on the active report, instantly transitioning the UI card forward.
   - Trigger a debounced query invalidation (`queryClient.invalidateQueries({ queryKey: ['screening-queue'] })`) to let the authoritative PostgreSQL state replenish the queue window.
3. **Completion Criteria**:
   - Rapidly screening 20 articles via keyboard shortcuts (`I` for Include, `E` for Exclude) never creates duplicate items or phantom empty pages.
   - A `409 screening_revision_conflict` cleanly resets only the affected item without breaking queue navigation.

---

### 4.6. Publication-Ready PRISMA Export: Headless Server Rasterization

#### The Defect
In [`png.ts`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/apps/web/src/lib/features/prisma/png.ts), PRISMA PNG exports rely on loading the SVG into a client-side `HTMLImageElement` and painting to a `<canvas>`. Output quality, DPI, and typography depend on client OS fonts and browser rendering bugs.

#### Target Implementation
1. **Server Export Endpoint**:
   - In [`crates/http-api/src/routes/exports.rs`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/http-api/src/routes/exports.rs), extend `ExportKind` to include `PrismaPng` and `PrismaPdf`:
     ```rust
     "prisma.png" => Self::PrismaPng,
     "prisma.pdf" => Self::PrismaPdf,
     ```
2. **Deterministic Rendering with Embedded Fonts**:
   - Use `resvg` in Rust to render the authoritative SVG produced by [`crates/application/src/prisma.rs`](file:///home/luzes/Documents/ambient-scribes/.worktrees/ui-human-touch/crates/application/src/prisma.rs) at 300 DPI with standard bundled fonts (Inter / Helvetica).
   - Return binary `image/png` or `application/pdf` with `Content-Disposition: attachment; filename="prisma-flowchart.png"`.
3. **Completion Criteria**:
   - Downloading PRISMA PNG from the UI hits `GET /projects/{id}/exports/prisma.png` and returns a crisp, publication-ready 300 DPI image that is identical across any client OS.

---

## 5. Phased Implementation Sequence

To minimize risk and deliver immediate value, implement the fixes in three distinct phases:

### Phase 1: High ROI Correctness & Persistence
1. **Article Catalog Filtering & Sorting** (Subsystem 4.1):
   - Update `crates/http-api/src/routes/articles.rs` with query parameters (`search`, `sort`, `min_internal`).
   - Run `pnpm run generate:api` to update generated client code.
   - Refactor `ProjectArticlesView.svelte` to remove in-memory filtering.
2. **PostgreSQL Workflow Storage** (Subsystem 4.3):
   - Add `automation_workflows` table migration.
   - Add `GET`/`PUT` endpoints in `crates/http-api/src/routes/automations.rs`.
   - Update `AutomationCenter.svelte` to save/load from server.

### Phase 2: Performance & Scalability
3. **Graph Layout Precomputation** (Subsystem 4.2):
   - Add `graph_layout_x` and `graph_layout_y` to `project_reports` schema.
   - Add deterministic 2D layout calculation to worker projection pass.
   - Update Sigma renderer in `project-graph-renderer.ts` to consume precomputed coordinates.
4. **Publication-Ready PRISMA Export** (Subsystem 4.6):
   - Add `resvg` rendering in `crates/http-api/src/routes/exports.rs`.
   - Update PRISMA download buttons to trigger server export.

### Phase 3: Resource Footprint & Reliability
5. **Document Page Streaming via Pdfium** (Subsystem 4.4):
   - Add page tile endpoint in `crates/http-api/src/routes/documents.rs`.
   - Replace `<canvas>` loop in `PdfViewer.svelte` with virtualized `<img>` tiles.
   - Remove `pdfjs-dist` from dependencies.
6. **Screening Queue Cache Simplification** (Subsystem 4.5):
   - Streamline `optimistic.ts` and rely on server window invalidation.

---

## 6. Verification and Test Strategy

Every phase must pass verification across all three repo tiers:

1. **Rust Backend Tests**:
   - `cargo test --workspace` must pass with 0 errors.
   - Add integration tests in `crates/http-api/tests/` for new query parameters, workflow persistence, and export formats.
2. **API Generation Integrity**:
   - Run `pnpm run generate:api` and ensure git diff shows clean, aligned models in `apps/web/src/lib/api/generated/`.
3. **Frontend Component & Unit Tests**:
   - `pnpm --filter @deepref/web test` must pass.
   - Visual tests via Playwright (`pnpm --filter @deepref/web test:visual`) must verify that graph views and document viewers render without layout shift.
4. **End-to-End Sanity Check**:
   - Start stack via `pnpm run dev` and test:
     - Searching and sorting across articles.
     - Saving a visual workflow and refreshing the browser on a new incognito window to verify persistence.
     - Viewing the citation graph with immediate zero-jank node placement.
