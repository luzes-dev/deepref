# Workflow editor and execution boundary

The workflow editor authors a versioned, plain-data workflow definition. Rete is its current browser adapter. The research graph explorer remains a separate Sigma/Graphology feature.

## Ownership

- `apps/web/src/lib/features/workflows/domain` owns workflow definitions, operation definitions, typed ports, validation, serialization and commands. It has no Rete, Svelte, DOM, generated API or application-store dependencies.
- `apps/web/src/lib/features/workflows/editor` owns client-only Rete integration, Svelte rendering, viewport, selection and reconciliation.
- `apps/web/src/lib/features/automations` owns the palette, inspector, browser draft persistence and existing built-in recipe settings.
- `@deepref/ui/flow` supplies renderer-independent presentation. It does not own workflow semantics or Rete mechanics.

The editor receives the canonical definition. A gesture becomes a domain command, whose accepted result is reconciled into the editor. Selection, hover and viewport are ephemeral. Node layout is saved separately from node configuration. Execution progress and outputs do not belong to the reusable definition.

## Persistence and existing automation execution

Before this migration, browser drafts used `version: 1` with XYFlow node and edge shapes. They were stored under `deepref:automation-graph:<project>:<definition>`. They were never sent to the automation API as executable recipes.

That product boundary remains. The current API saves name, trigger and status for a built-in maintenance recipe. Running that recipe does not execute arbitrary locally authored graphs. The editor must continue to explain this distinction beside its save/run controls.

The new workflow document uses `schemaVersion`, stable node/connection IDs, stable operation kind and definition version, JSON configuration, explicit source/target port IDs and optional layout. Deserialization validates the boundary. Future schema versions, unavailable operations, invalid configuration and incompatible connections produce explicit errors rather than partial graphs.

## Node and port contracts

A node instance stores its stable kind and definition version. The registry supplies its title, configuration validator, default configuration and explicit input/output ports. Port IDs and data type IDs are strings in JSON and branded types in TypeScript. Connections name both endpoint node IDs and port IDs; type compatibility and input cardinality are checked by the domain before the renderer accepts an edge. Diagram appearance does not determine compatibility.

The registry supports additional operation definitions without importing editor classes. Assistant previews use a separate application-owned registry of explanatory nodes; their runtime progress remains outside the workflow document. A cycle check is separate from structural validity so a future execution planner can impose its own acyclic constraint.

## Rust execution authority

The backend validates registered workflow nodes and ports before execution. Do not serialize Rete objects or execute JavaScript supplied in node configuration: node kind selects an allowlisted operation, and configuration contains data.

Workflow execution shares `durable_runs`, `durable_steps`, and `durable_attempts` with purpose-built automations and compiled reviews. `workflow_run_details` stores the graph and trigger snapshot; `workflow_step_details` stores node identity, iteration, inputs, fired ports and wait state. These detail tables do not own lifecycle status or leases. The generic job queue dispatches `durable_run_step` to the workflow handler and remains the lease authority.

Every node attempt requires a live job lease matching job identity, owner and attempt. Waiting queues another look at the same canonical step; branch and iteration planning consumes its typed details. Cancellation locks the run before its steps, closes unfinished steps and attempts, and prevents late outcomes from scheduling successors or another wait poll. Public run and node UUIDs remain stable.

Slack, arbitrary HTTP, webhook and inbound-email nodes are parked by default.
The catalog preserves every definition and exposes runtime `available` metadata;
the editor retains all definitions for saved-graph rendering and offers only
available definitions when adding a block. Disabled graphs cannot be newly
saved, published, enabled or started, and queued worker executions fail before
any network side effect. `DEEPREF_WORKFLOW_NICHE_NODES=true` opts into these
nodes through the shared runtime configuration. Outgoing email is not parked.

## Renderer dependencies

The integration follows the [official Svelte renderer guide](https://retejs.org/docs/guides/renderers/svelte/), with `rete-svelte-plugin/5` dynamically imported through the client editor lifecycle. Installed versions at migration: Rete 2.0.6, area 2.3.2, connection 2.0.5, Svelte renderer 2.1.2 render-utils 2.0.3 and minimap 2.0.3. Package manifests report MIT licenses and a Svelte peer range that includes the installed Svelte 5.57.0.

The Rete and svelte-preprocess install scripts only print informational banners; both are explicitly disabled in pnpm's build policy. No React renderer or application watermark is required. Existing Storybook/Vite peer warnings predate this integration and must not be mistaken for evidence of Rete incompatibility.

The official renderer's presets contain SCSS. The web application's `svelte.config.js` enables [`vitePreprocess()`](https://svelte.dev/docs/kit/integrations#vitePreprocess) so these styles compile with the installed Sass package. The first integrated production build exposed this missing configuration; enabling the preprocessor resolved that build failure. This does not require a custom renderer bridge.

## Validation record

The former `Patterns/Workflow canvas` story and its XYFlow-specific viewport tests are replaced deliberately. `Patterns/Workflow nodes` renders independent presentation states in Storybook. The viewport contract now lives with the application renderer in `apps/web/tests/visual/workflow-viewport.spec.ts`: wheel/trackpad synchronization, reduced-motion controls and consistent initial/manual fit across both themes and widths. Application tests exercise editing and persistence at their owning feature boundary.

Implementation and review are in progress. Final commands and observed results will be recorded here after integration; no pending check is claimed as passing.
