# Deepref UI

A compact visual system for evidence and research work. Warm paper and charcoal surfaces, teal actions, IBM Plex Sans controls, and Source Serif 4 reading content keep long sessions readable without turning every section into a card.

Import primitives from explicit subpaths (`@deepref/ui/button`, `@deepref/ui/field`) and generic composition from `@deepref/ui/layout`. Import `@deepref/ui/styles.css` once in the host stylesheet. The public `theme.css` path is a compatibility export of the same stylesheet; do not import both.

## Tokens and composition

`src/styles/styles.css` has three layers: a small private foundation palette and rhythm, light/dark semantic roles, then Tailwind mappings consumed by components. Components should use roles rather than raw palette utilities.

| Need                                                  | Role / convention                                                               |
| ----------------------------------------------------- | ------------------------------------------------------------------------------- |
| Page, ordinary surface, recessed region, overlay      | `background`, `card`, `surface-inset`, `popover`                                |
| Body, secondary copy, metadata                        | `foreground`, `text-secondary`, `muted-foreground`                              |
| Quiet separator, ordinary boundary, emphatic boundary | `border-subtle`, `border`, `border-strong`                                      |
| Editable control boundary                             | `input`                                                                         |
| Main action and pressed feedback                      | `primary`, `primary-hover`, `primary-active`                                    |
| Selection and neutral interaction                     | `selection`, `selection-foreground`, `interactive-hover`, `interactive-active`  |
| Feedback                                              | `success`, `warning`, `destructive`, `info`, each with `-surface` and `-border` |
| Keyboard focus                                        | `ring`: opaque 2px outline, 2px offset                                          |
| Data series                                           | `chart-1` through `chart-5`; independent of status meaning                      |

Use the existing four-pixel Tailwind rhythm. Standard controls are 36px high; compact variants are for dense toolbars. Page padding is fluid from 16 to 32px. Section gaps are 16px. Keep control labels in the sans family; reserve the serif family for sustained reading. Avoid using disabled text for explanatory copy: disabled states are intentionally subdued.

`PageFrame` owns the shell and skip link. `PageHeader` wraps long titles and actions. `PageToolbar` wraps filters and trailing controls. `Surface` distinguishes plain sections, default working surfaces, subtle context, inset evidence, and raised temporary content; its caller owns padding. Use headings and spacing before adding another surface. `MetricTile` keeps values compact and tabular. `StatePanel` provides labelled empty, loading, success, warning and error feedback.

Use Badge for compact status and Alert for actionable explanation. Status should include words or an icon, not color alone. Destructive actions need explicit names. LoadingButton preserves its accessible name, disables repeat submission and exposes `aria-busy`.

Dialogs need a Title and Description inside Content. Keep long content scrollable and footer actions reachable. Use a Sheet for adjacent inspection and a Drawer for a compact bottom interaction. Form controls require associated labels; connect errors with `aria-describedby` and set `aria-invalid`. Tables retain horizontal scrolling at narrow widths rather than squeezing structured columns into unreadable text.

Motion uses 120ms interactions, 180ms ordinary transitions and 220ms panels. Reduced-motion preferences suppress animation; terminal text also appears without typing delays.

## Storybook

Run `pnpm dev:storybook` from the repository root. The theme toolbar switches deliberate light and dark themes.

- **Foundations** explains surfaces, type, status and chart roles.
- **Primitives** demonstrates actions, labelled forms, status, tables, overlays, navigation, supporting controls and sequences.
- **Patterns** demonstrates constrained page composition, responsive shell, state panels and a neutral workflow canvas.

Stories include disabled/loading controls, long labels, validation, empty filtering, many actions, short-height dialogs and keyboard interactions. Fixtures are local presentation data. They must not import application stores, API models or query clients.

## Validation and screenshot review

From the root: `pnpm check`, `pnpm test`, `pnpm lint`, `pnpm build`, `pnpm build:storybook`.

Component browser checks: `pnpm --dir packages/ui test:visual`. The matrix covers desktop and narrow light/dark views, eight high-information screenshots and focused keyboard contracts. Application visual/axe coverage remains in `apps/web/tests/visual`.

To inspect proposed screenshot changes without accepting them, run with `REVIEW_CAPTURE=1`. Review the generated `review.png` captures in both themes and widths, correct defects, then accept only reviewed files. A baseline update command alone is not visual validation.

The package owns presentation and generic behavior. It must never import application API clients, TanStack Query, feature modules, routes, application stores, `$app` or web `$lib` aliases. Application-specific compatibility layout styles belong to the application stylesheet.
