# Deepref Agent Guidelines & Engineering Standards

Repository guidance for AI agents and human contributors working across the Deepref monorepo (`apps/web`, `packages/ui`, `packages/core`).

---

## 1. Architecture & Design System

Deepref enforces a strict design system architecture between `@deepref/ui` (reusable primitives and design tokens) and `apps/web` (application features and workflows).

### Design Tokens & Typography

Theme tokens are declared in [`packages/ui/src/styles/styles.css`](file:///home/luzes/.t3/worktrees/ambient-scribes/t3code-729f9577/packages/ui/src/styles/styles.css) and exposed through Tailwind CSS v4 `@theme inline`:

- **Typography Scale**:
  - Headings & reading: Standard scale (`text-xs`, `text-sm`, `text-base`, `text-lg`, `text-xl`, etc.) with `font-sans` (IBM Plex Sans) or `font-serif` (Source Serif 4).
  - Microcopy & kickers:
    - `text-2xs` (0.6875rem / 11px, line-height: 0.9375rem)
    - `text-3xs` (0.625rem / 10px, line-height: 0.8125rem)
    - `text-4xs` (0.5625rem / 9px, line-height: 0.75rem)
  - Letter Spacing (Tracking):
    - `tracking-snug-caps` (`0.08em`)
    - `tracking-caps` (`0.12em`)
    - `tracking-kicker` (`0.16em`)
    - `tracking-wide-caps` (`0.18em`)
    - `tracking-widest-caps` (`0.2em`)
  - Numeric alignment: Always use native `tabular-nums` (never arbitrary `[font-variant-numeric:...]`).
- **Surface & Shadows**:
  - Semantic shadows: `shadow-xs`, `shadow-sm`, `shadow-md`, `shadow-lg`, and accent shadow `shadow-inset-accent` (`inset 3px 0 0 var(--primary)`).
- **Custom Utilities**:
  - Registered via `@utility` in [`packages/ui/src/styles/styles.css`](file:///home/luzes/.t3/worktrees/ambient-scribes/t3code-729f9577/packages/ui/src/styles/styles.css) and [`apps/web/src/routes/layout.css`](file:///home/luzes/.t3/worktrees/ambient-scribes/t3code-729f9577/apps/web/src/routes/layout.css):
    - `editorial-title`: Sans, semi-bold (`font-weight: 600`), negative tracking (`-0.02em`).
    - `disclosure`: Border bottom `var(--border)` with vertical padding `0.75rem`.

### Primitive Variants & Sizing

Components own their internal presentation and provide formal variant/size contracts:

- **Badge** ([`packages/ui/src/primitives/badge`](file:///home/luzes/.t3/worktrees/ambient-scribes/t3code-729f9577/packages/ui/src/primitives/badge/badge.svelte)):
  - Variants: `default`, `secondary`, `destructive`, `outline`, `success`, `warning`, `info`, `ghost`, `link`.
  - Sizes: `default` (h-5.5, px-2), `sm` (h-4.5, px-1.5, text-2xs), `xs` (h-4, px-1, text-3xs).
- **BubbleRoot** ([`packages/ui/src/primitives/bubble`](file:///home/luzes/.t3/worktrees/ambient-scribes/t3code-729f9577/packages/ui/src/primitives/bubble/bubble-root.svelte)):
  - Variants: `default`, `muted`, `accent`.
  - Sizes: `default` (p-3 rounded-lg), `compact` (px-2.5 py-1.5 rounded-md).
- **Button** ([`packages/ui/src/primitives/button`](file:///home/luzes/.t3/worktrees/ambient-scribes/t3code-729f9577/packages/ui/src/primitives/button/button.svelte)):
  - Variants: `default`, `outline`, `secondary`, `ghost`, `destructive`, `link`.
  - Sizes: `default`, `xs`, `sm`, `lg`, `icon`, `icon-xs`, `icon-sm`, `icon-lg`.

---

## 2. `@shadcn/lint` Enforcement Rules

`apps/web` integrates `@shadcn/lint` through ESLint ([`apps/web/eslint.config.js`](file:///home/luzes/.t3/worktrees/ambient-scribes/t3code-729f9577/apps/web/eslint.config.js)). All code written must comply with the following contracts:

### Rule 1: `shadcn/no-raw-colors` (Severity: Error)

Never use hardcoded color utility classes (e.g. `bg-blue-500`, `text-zinc-600`, `border-gray-200`). Always use semantic color tokens:

- Backgrounds: `bg-background`, `bg-card`, `bg-muted`, `bg-primary`, `bg-secondary`, `bg-accent`, `bg-destructive`, `bg-success`, `bg-warning`, `bg-info`.
- Text: `text-foreground`, `text-muted-foreground`, `text-primary-foreground`, `text-destructive-foreground`, `text-success`, `text-warning`.
- Borders: `border-border`, `border-input`, `border-primary`, `border-muted`.

### Rule 2: `shadcn/no-arbitrary-values` (Severity: Error)

Disallow arbitrary Tailwind classes (`[value]`), except for dynamic layouts (`w-[...]`, `grid-cols-[...]`).

- **Forbidden**: `[font-variant-numeric:tabular-nums]`, `tracking-[0.2em]`, `backdrop-blur-[1px]`.
- **Target**: `tabular-nums`, `tracking-widest-caps`, `backdrop-blur-xs`.

### Rule 3: `shadcn/no-unknown-classes` (Severity: Error)

Every CSS utility used on an element must be recognized by Tailwind CSS v4.

- Custom utilities must be defined using the `@utility <class-name>` directive in the root CSS entry point ([`apps/web/src/routes/layout.css`](file:///home/luzes/.t3/worktrees/ambient-scribes/t3code-729f9577/apps/web/src/routes/layout.css)) or `@deepref/ui/styles.css`.

### Rule 4: `shadcn/no-inline-styles` (Severity: Error)

Never use the HTML/Svelte `style="..."` attribute for static CSS styling.

- Inline styles are permitted **only** for dynamic CSS variables (`--*`), spatial coordinates/dimensions computed at runtime (`left`, `top`, `width`, `height`, `transform`), and browser view transitions (`view-transition-name`).

### Rule 5: `shadcn/no-restyle` (Severity: Warning)

Avoid ad-hoc inline restyling of design system primitives.

- When an element requires a specific visual treatment, select an existing primitive variant or declare a new variant in `@deepref/ui`.
- Structural containers (cards, dialogs, drawers, sheets, tables, dropdowns, popovers) allow layout customization (margins, sizing, flex/grid alignment).

---

## 3. Verification & Quality Gates

Run the following test and lint verification before completing any task:

```bash
# 1. Type checking across the workspace
pnpm check

# 2. Unit and component tests
pnpm test

# 3. Format and lint checks (Prettier & ESLint with @shadcn/lint)
pnpm lint
```

When editing Svelte or CSS files, ensure formatting is clean by running:

```bash
pnpm --filter @deepref/web exec prettier --write <modified-files>
pnpm --filter @deepref/ui exec prettier --write <modified-files>
```
