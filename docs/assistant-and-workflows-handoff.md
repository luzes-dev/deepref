# Handoff: Unified Workflows Hub & Agentic Assistant Chatbot

> **Authoritative Handoff & Architecture Document**  
> **Audience:** Autonomous Coding Agent & UI Specialist Reviewer  
> **Status:** COMPLETE (all 4 phases implemented and verified; see §5 for the final session record)  
> **Scope:** Architecture, UI design decisions, component contracts, backend persistence, agent loop, and handoff execution steps.

---

## 1. Executive Summary & Objective

The DeepRef project has evolved from two overlapping screens into a decoupled, purpose-built architecture:
1. **Workflows Hub (`/automations`)**: Consolidates all structured tool executions into **Predefined Automation Recipes** alongside visual multi-step DAG workflows.
   - **Strict Invariant**: All recipe executions MUST run as asynchronous, durable worker jobs backed by PostgreSQL lease fencing and idempotency keys. There is **NO synchronous execution bypass**. Every invocation generates a tracked `AutomationRunDto`.
2. **Conversational Chat Assistant (`/assistant`)**: Transforms the legacy one-shot tool parameter form into an agentic, multi-turn AI chatbot with project-wide capabilities.
   - **Strict Invariant**: AI Never Directly Mutates Scientific Truth. AI tools produce typed **Proposals** (`propose_screening_decision`, `propose_duplicate_merge`, `propose_extraction`, `propose_appraisal_answer`) that enter human review queues.
   - Built exclusively using first-party **base `shadcn-svelte` primitives** (`message`, `bubble`, `marker`, `attachment`, `avatar`, `input-group`, `scroll-area`) in `@deepref/ui`.

---

## 2. Implementation Progress Dashboard

| Phase | Description | Status | Verification & Artifacts |
| :--- | :--- | :--- | :--- |
| **Phase 1** | **Base Chat UI Primitives & Patterns (`@deepref/ui`)** | **COMPLETED** | 26 unit tests passing (`chat.spec.ts`, `chat-primitives.spec.ts`). Exported in `@deepref/ui`. |
| **Phase 2** | **Workflows Hub & Predefined Recipes (`/automations`)** | **COMPLETED** | `recipes.ts` (14 recipes) + `RecipeLibrary.svelte` + `RecipeLaunchModal.svelte` mounted in `AutomationCenter.svelte` behind a hub-view toggle. |
| **Phase 3** | **Backend Chat Persistence & Streaming Agent Loop** | **COMPLETED** | `0023_assistant_chat.sql`, `crates/postgres/src/assistant.rs`, `crates/ai/src/assistant.rs`, plus SSE chat + conversation routes in `crates/http-api/src/routes/assistant.rs` (25 http-api lib tests green). |
| **Phase 4** | **Frontend Chatbot Interface (`/assistant`)** | **COMPLETED** | `ProjectChatAssistant.svelte` + `chat-api.ts` + `$lib/api/assistant-stream.ts` (SSE) + rewritten e2e spec; web check/test/lint green. |

---

## 3. UI Specialist Deep-Dive: Design Decisions & Component Hierarchy

This section documents the specific UI/UX, styling, accessibility, and component design choices made for the UI specialist agent.

### 3.1 Design System Alignment & Anti-Slop Principles
- **No Third-Party Extras Bloat**: Rather than installing monolithic chat component libraries with conflicting Tailwind versions, we built native primitives in [`packages/ui/src/primitives/`](../packages/ui/src/primitives/) following base `shadcn-svelte` design tokens, Bits UI integration, and Tailwind CSS v4 variables.
- **Svelte 5 Runes Native**: Built completely using Svelte 5 runes (`$props`, `$bindable`, `$derived`, `$state`), `WithElementRef` prop typings, and `data-slot` selectors for scoped styling.
- **Prose & Density Hierarchy**: Clean editorial styling using Tailwind typography classes with tight leading, subtle muted backgrounds, and zero unnecessary decorative borders.

### 3.2 Chat Primitives Architecture (`packages/ui/src/primitives/`)

#### 1. `Message` (`Message.Root`, `Message.Avatar`, `Message.Content`, `Message.Header`, `Message.Footer`, `Message.Group`)
- **Container Semantics**:
  - `Message.Group`: `<div role="log" aria-live="polite" data-slot="message-group">` wrapping all conversation turns.
  - `Message.Root`: Flex container with `align="start"` (bot/assistant) or `align="end"` (user).
- **Layout & Alignment**:
  - User messages: right-aligned (`justify-end`), avatar on the right, bubble background styled with `bg-primary text-primary-foreground`.
  - Assistant messages: left-aligned (`justify-start`), bot avatar pinned to top/bottom (`self-start` or `self-end`), content stacked vertically with metadata header (`DeepRef Assistant`, model badge, timestamp).
- **Metadata Footers**:
  - Contains token counts, input hashes, latency, and quick copy action slots without visual clutter.

#### 2. `Bubble` (`Bubble.Root`, `Bubble.Content`, `Bubble.Group`, `Bubble.Reactions`)
- **Variants Hierarchy**:
  - `variant="default"`: Solid high-contrast background (`bg-primary text-primary-foreground`) for user queries.
  - `variant="muted"`: Subtle surface (`bg-muted/60 text-foreground border border-border/40`) for assistant reasoning and narrative responses.
  - `variant="outline"`: Transparent framed surface (`border border-border bg-card text-card-foreground`) reserved for consequential cards (Proposals and Tool Calls).
- **Grouping**:
  - `Bubble.Group`: Seamlessly stacks consecutive reasoning text, tool executions, and scientific answers within a single assistant turn without repeating avatars or headers.

#### 3. `Marker` (`Marker.Root`, `Marker.Content`)
- **Live Status Indicator**:
  - Designed for agent planning and tool execution phases.
  - Accessibility: `role="status"` and `aria-live="polite"`.
  - Styling: Features a subtle pulse/shimmer animation (`class="animate-pulse flex items-center gap-2 text-xs font-mono text-muted-foreground"`).
  - Displays dynamic messages (e.g. `DeepRef Assistant is executing search_project_reports...`).

#### 4. `Attachment` (`Attachment.Root`, `Attachment.Preview`, `Attachment.Name`)
- **Evidence & Citation Pills**:
  - Inline evidence tags representing project protocol versions, document blocks, and report IDs.
  - Rounded-full pill styling (`rounded-full border border-border/80 bg-background/80 px-2.5 py-0.5 text-xs font-medium hover:bg-muted/60 transition-colors`).
  - Supports click actions to inspect the source citation or document block in the project viewer.

#### 5. `Avatar` (`Avatar.Root`, `Avatar.Image`, `Avatar.Fallback`)
- Bits UI-backed avatar container with rounded-full geometry, image error handling, and high-contrast typographic fallbacks.

### 3.3 Chat Domain Patterns (`packages/ui/src/patterns/chat/`)

#### `ToolCallCard.svelte`
- Embedded collapsible card within an assistant `Bubble.Group`.
- **States**:
  - `running`: Animated spinner badge (`variant="outline" class="border-amber-500/40 text-amber-600 dark:text-amber-400"`).
  - `completed`: Success check icon (`variant="secondary" class="text-emerald-600 dark:text-emerald-400"`).
  - `failed`: Destructive badge (`variant="destructive"`).
- Monospace tool name tag, collapsible JSON/formatted argument drawer, and summarized outputs.

#### `ProposalCard.svelte`
- Distinctive high-priority card indicating an AI-generated proposal.
- Clearly displays:
  - Target entity (e.g. `Report: 1234...`, `Study: 5678...`).
  - Proposal type badge (`Screening Decision`, `Duplicate Merge`, `Classification`, `Extraction`, `Appraisal Answer`).
  - Rationale summary snippet.
  - Primary CTA button: **"Review in Queue"** linking directly to the corresponding reviewer interface with pre-filtered query parameters.

### 3.4 Workflows Hub: Recipe Library UI Decisions (`/automations`)
- **Tabbed Layout**: Clean toggle between "Custom Workflows" and "Predefined Recipes" in [`AutomationCenter.svelte`](../apps/web/src/lib/features/automations/components/AutomationCenter.svelte).
- **Categorization**: Grouped into 4 clinical evidence stages:
  1. *Screening* (Single Report Screening, Eligibility Check)
  2. *Studies & Synthesis* (Study Grouping, Study Classification)
  3. *Document Analysis* (Document Extraction, Block Reading, Document Search)
  4. *Maintenance* (Project Metrics Maintenance, Duplicate Detection)
- **Parameter Modal**:
  - Follows strict `<Field.FieldGroup>`, `<Field.Field>`, `<Field.FieldLabel>`, `<Field.FieldError>` semantics with `data-invalid` and `aria-invalid` attributes.
  - Clear help text and character bounds.
  - "Fork into Custom Workflow" button transitions the recipe into the visual node editor (`AutomationEditor.svelte`).

### 3.5 Final Session UI Decisions (Recipe Library & Chatbot)

**Hub toggle (`AutomationCenter.svelte`)**
- Segmented pill toggle (`rounded-full` track on `bg-muted/40`, active segment `bg-background` + `shadow-2xs`) between "Automations" and "Recipe Library". Defaults to Automations so the existing e2e contract (`automation-manager` visible, heading "Automations") is untouched. `aria-pressed` per segment; testids `hub-view-toggle` / `hub-view-automations` / `hub-view-recipes`.

**Recipe Library (`RecipeLibrary.svelte`)**
- Cards grouped by the four clinical stages, each stage an `aria-labelledby` section with an `h2` and a one-line editorial description. Cards reuse the hub's card language (`rounded-xl border-border/80 bg-card`, `hover:bg-muted/25`) in a responsive `sm:grid-cols-2 xl:grid-cols-3` grid.
- Icon container `size-9 rounded-lg bg-primary/10 text-primary`; category shown as an outline `Badge` (11px). Primary action is a `size="sm"` Button ("Run recipe"; read-only recipes say "Run inspection"), plus a ghost "Fork" with `GitForkIcon`.
- Per-recipe lucide icon names (from `recipes.ts`) resolve through a typed local map with a `WrenchIcon` fallback.

**Recipe Launch Modal (`RecipeLaunchModal.svelte`)**
- `sm:max-w-lg` Modal with bordered Header; the form uses `Field.FieldGroup` → `Field.Field` (with `data-invalid`) → `Field.FieldLabel` → control → `Field.FieldError`/`Field.FieldDescription`. Errors appear only after a failed submit and clear per-field as the user edits.
- Controls by field kind: `uuid`/`text` → `Input`, `integer` → number `Input` with min/max, `uuid-list` → 4-row `Textarea`, `stage` → `Select` ("Title & abstract" / "Full text").
- Submit flow: ensure a `manual`/`active` definition exists for the recipe route (reuse or PUT configure), then trigger with a fresh `Idempotency-Key`. Success swaps the form for a calm "Run queued" Alert with the short run id; a destructive Alert reports failures. Success keeps the modal open; closing it switches the hub back to Automations with Recent activity forced open (`runsForceOpen`) and the run preselected.

**Chatbot (`ProjectChatAssistant.svelte`)**
- Three-region layout: left conversation sidebar (`w-72`, hidden below `md` behind a `PanelLeftIcon` overlay toggle), center feed, bottom composer — inside the same full-height `bg-background` shell as other feature pages.
- Sidebar: uppercase tracked "Conversations" label, secondary "New Chat" button, hover-revealed destructive delete per thread (window.confirm), active thread `bg-muted/50`, two-line rows (title truncate + short date).
- Feed: `ScrollArea` (viewport ref bound for stick-to-bottom auto-scroll with a ~200px user-scroll-up guard) containing `Message.Group` (`role="log"` `aria-live="polite"`, `max-w-3xl` centered).
  - User turn: `Message.Root align="end"` + `Bubble.Root variant="default"` (`max-w-[85%]`, `px-4 py-2.5`, `text-sm leading-relaxed whitespace-pre-wrap`) + `size-7` avatar with "U" fallback.
  - Assistant turn: `Message.Root align="start"` + `Message.Avatar` (bot glyph on `bg-primary/10`) + `Message.Header` ("DeepRef Assistant" + time) + `Bubble.Group` holding: muted text bubble (only when there is text), `ToolCallCard`s (running → completed/failed, collapsible args/output), `ProposalCard`s (kind from tool name, target review-run id, "Review in Queue" → `/projects/{id}/{screening|deduplication|studies|extraction|appraisal}`), and a live `Marker.Root` ("DeepRef Assistant is executing {tool}…" / "…is thinking…").
  - Evidence: up to 8 deduped `Attachment` pills (`Protocol/Report/Block/Record` + short uuid) harvested from tool outputs; token totals render in `Message.Footer` (11px).
- Empty state: dashed-border panel with bot glyph and a grounded description ("…never changes scientific records directly").
- Composer: `InputGroup.Root` card (`rounded-xl border-border/80 bg-card p-2`) with auto-resizing `InputGroup.Textarea` (max ~192px, `rows=1`), send `size="icon"` button with `SendHorizontalIcon` (spinner while streaming), Enter submits / Shift+Enter newline, hint line underneath. Inline destructive Alert for stream errors above the composer.
- Transport lives in `$lib/api/assistant-stream.ts` (the only place raw `fetch` is permitted by the eslint boundary): pure SSE frame parser, typed event union, `AssistantStreamError` with status. Stream logic mutates the Svelte 5 state proxy (not the raw object) so every token/tool frame is reactive.


---

## 4. Work Accomplished in Current Session

### Phase 1: Base Chat UI Primitives (`@deepref/ui`) — **DONE**
1. Implemented all primitives in [`packages/ui/src/primitives/`](../packages/ui/src/primitives/):
   - `avatar/`: Avatar Root, Image, Fallback wrapping Bits UI.
   - `message/`: Message Root (with `align="start" | "end"`), Avatar, Content, Header, Footer, Group.
   - `bubble/`: Bubble Root (variants: `default`, `muted`, `outline`), Content, Group, Reactions.
   - `marker/`: Marker Root (`role="status"`, animated shimmer), Content.
   - `attachment/`: Attachment Root, Preview, Name.
2. Implemented domain patterns in [`packages/ui/src/patterns/chat/`](../packages/ui/src/patterns/chat/):
   - `ToolCallCard.svelte`: Collapsible card with status badges, tool name, arguments, and result previews.
   - `ProposalCard.svelte`: Actionable proposal summary with review destination link.
3. Exported all primitives and patterns in `packages/ui/src/index.ts` and `packages/ui/package.json`.
4. Created test suites:
   - `packages/ui/src/primitives/chat-primitives.spec.ts` (11 tests passed)
   - `packages/ui/src/patterns/chat/chat.spec.ts` (8 tests passed)
   - Total 26 tests in `@deepref/ui` passed with 0 errors.

### Phase 2: Workflows Predefined Recipes — **CORE DONE**
1. Created [`apps/web/src/lib/features/automations/recipes.ts`](../apps/web/src/lib/features/automations/recipes.ts):
   - Ported all 14 assistant tool definitions into structured `PredefinedRecipe` definitions.
   - Configured fields schemas, validation rules, default parameters, categories, and backend recipe routes (`review_screening.v1`, `review_duplicate_detection.v1`, `review_study_classification.v1`, `review_study_grouping.v1`, `review_appraisal_prefill.v1`, `review_data_extraction.v1`, `project_maintenance.v1`).

### Phase 3: Backend Persistence & Agent Engine — **CORE DONE**
1. Database Migration:
   - Created [`crates/postgres/migrations/0023_assistant_chat.sql`](../crates/postgres/migrations/0023_assistant_chat.sql):
     - `assistant_conversations` table with project FK and updated_at indexes.
     - `assistant_messages` table with role check, JSONB tool_calls, tool_results, metadata.
2. PostgreSQL persistence in [`crates/postgres/src/assistant.rs`](../crates/postgres/src/assistant.rs):
   - `create_assistant_conversation`, `list_assistant_conversations`, `get_assistant_conversation`, `delete_assistant_conversation`.
   - `append_assistant_message`, `list_assistant_messages`.
   - Re-exported in `crates/postgres/src/lib.rs`.
3. Agent Declarations & Policy in [`crates/ai/src/assistant.rs`](../crates/ai/src/assistant.rs):
   - Multi-turn conversation types (`AssistantChatMessage`, `AssistantRole`, `AssistantToolCall`, `AssistantToolResult`).
   - Function declarations for all 14 tools + `trigger_workflow`.
   - Policy verification via `PolicyEngine` ensuring no tool bypasses project scope or actor authority tiers.
   - Proposal invariant preserved: Proposal tools schedule review runs, never directly mutating records.

---

## 5. Final Session Record (Completing Agent)

The three remaining tasks were implemented and verified end to end:

### Baseline repairs (found broken on arrival)
- `crates/ai/src/assistant.rs` did not compile: 14 stray line-continuation backslashes inside `json!(` macros, `AgentTool::from_name_and_args` was private, and the policy call site used a non-existent `AgentTool::is_read`. Fixed; a project-scope precondition now returns `AgentToolError::InvalidProjectScope` for cross-project tools (matching the test contract) while genuine policy denials remain `Forbidden`.
- Pre-existing type/lint failures in `AutomationEditor.svelte` (`paletteCanonicalKind` narrowing), `automation-workflow.ts`, `recipes.ts`, and formatting drift were also repaired so the full verification suite is green.

### Task 1: Backend HTTP SSE & conversation routes — DONE
`crates/http-api/src/routes/assistant.rs` (registered in `routes/mod.rs`):
- `GET /projects/{project_id}/assistant/conversations` → 200 list (ordered by recent activity).
- `POST /projects/{project_id}/assistant/conversations` `{title}` → 201; blank/>500-char titles → 400.
- `GET /projects/{project_id}/assistant/conversations/{conversation_id}/messages` → 200 chronological history (404 when not in project).
- `DELETE /projects/{project_id}/assistant/conversations/{conversation_id}` → 204.
- `POST /projects/{project_id}/assistant/chat` `{conversation_id, message}` → 200 `text/event-stream`.
  - SSE wire contract: `event: token|tool_start|tool_complete|proposal_created|done|error` + `data: <json payload>` (payload is the inner data object only). Authorization failures before the first frame map to proper HTTP statuses (400/403); mid-stream failures emit a terminal `error` frame.
  - Persistence: user message appended before the turn; assistant message (content + tool_calls + tool_results + metadata) appended after `run_assistant_react_turn` completes; conversation `updated_at` touched.
  - Tools dispatched through `ConversationAgentDispatcher` reusing the legacy `execute_read`/`execute_proposal` helpers, plus `trigger_workflow` via `start_automation_manually` (fresh idempotency key per call).
  - The OpenAPI path-existence test in `routes/mod.rs` now covers the new paths.

### Task 2: Recipe Library mounted into `/automations` — DONE
- `AutomationCenter.svelte` grew a segmented hub-view toggle (`Automations` | `Recipe Library`, default Automations; e2e contract preserved: `automation-manager`, heading "Automations", default view).
- `RecipeLibrary.svelte`: cards grouped by the four clinical stages, per-recipe lucide icon map, category badges, `Run recipe` / `Run inspection` (read-only recipes) and `Fork` actions.
- `RecipeLaunchModal.svelte`: Field primitives with `data-invalid`/`aria-invalid`, per-kind inputs (uuid/text/integer/uuid-list/stage Select), validation via `validateRecipeParameters`, ensure-definition-then-trigger flow (`configureAutomationDefinition` PUT + `triggerAutomationManually` with fresh `Idempotency-Key`), inline queued-run success state, and `View activity` switching the hub back to Automations with Recent activity opened (`runsForceOpen`) and the run highlighted.
- `Fork` opens `AutomationEditor` in add mode with a recipe-derived, de-duplicated name.
- Known API limitation (unchanged): `StartAutomationRequest` carries only `definition_id`, so collected parameters are validated client-side but the trigger endpoint has no parameter channel yet.

### Task 3: Agentic chatbot frontend — DONE
- `apps/web/src/lib/api/assistant-stream.ts`: SSE transport (raw fetch allowed only in the api layer per the forbidden-fetch eslint boundary) + pure, unit-tested frame parser; typed `AssistantChatStreamEvent` union; `AssistantStreamError` with HTTP status.
- `apps/web/src/lib/features/assistant/chat-api.ts`: typed conversation CRUD via `customFetch`, proposal-tool → review-queue mapping (`reviewDestinationForTool` / `reviewQueuePathForTool`), conversation-title derivation.
- `apps/web/src/lib/features/assistant/components/ProjectChatAssistant.svelte`: the full chat surface (details in §3.5).
- `apps/web/src/routes/projects/[projectId]/assistant/+page.svelte` renders `ProjectChatAssistant`; `page.svelte.e2e.ts` rewritten for the chat contract (history rendering, SSE streaming with tool/proposal frames, title derivation, delete confirmation, viewport bounds).

### Verification (all green at close)
```bash
pnpm --filter @deepref/ui check   # 0 errors
pnpm --filter @deepref/ui test    # 26 passed
pnpm --filter web check           # 0 errors
pnpm --filter web test            # 163 passed (41 files)
pnpm --filter web lint            # prettier + eslint clean
SQLX_OFFLINE=true cargo check --workspace          # clean
cargo test -p deepref-ai                           # 35 passed
SQLX_OFFLINE=true cargo test -p deepref-http-api --lib   # 25 passed
```

## 6. Verification Commands for the Incoming Agent

---

## 6. Verification Commands for the Incoming Agent

```bash
# 1. Check UI package
pnpm --filter @deepref/ui check
pnpm --filter @deepref/ui test

# 2. Check web package
pnpm --filter web check
pnpm --filter web test

# 3. Check Rust backend
SQLX_OFFLINE=true cargo check --workspace
cargo test -p deepref-ai
SQLX_OFFLINE=true cargo test -p deepref-http-api --lib
```
