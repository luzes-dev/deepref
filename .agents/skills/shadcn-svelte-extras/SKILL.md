---
name: shadcn-svelte-extras
description: Guide to shadcn-svelte-extras — the community library that extends shadcn-svelte with 33 components, 3 Svelte actions, and 9 Svelte hooks for Svelte 5 + SvelteKit. Use this whenever the user is in a SvelteKit or Svelte 5 app and asks for UI primitives like buttons, chat, code blocks, emoji pickers, file drop zones, image croppers, IPv4/phone inputs, language switchers, light switches, modals, NLP date inputs, number fields, password fields, package-manager command blocks, rename inputs, snippets, split buttons, star ratings, steppers, tag inputs, terminals, theme selectors, TOC, tree views, underline tabs, or windows. Also trigger on jsrepo, shadcn-svelte add, npx jsrepo add @ieedan/shadcn-svelte-extras, mode-watcher, or asks about installation, composition, or prop APIs for any extra. Covers Svelte 5 runes, $bindable props, Snippet children, and bits-ui forwarding.
---

# shadcn-svelte-extras

## What this is

`shadcn-svelte-extras` is a community library by `ieedan` that fills the gaps left by shadcn-svelte's built-in components. It is **not** a port — it ships original, composable primitives designed to match the look, feel, and quality of shadcn-svelte.

Components are **not** npm packages. They are distributed as copy-pastable source files. You install them by running a CLI that drops `.svelte` and `.ts` files into your project's `$lib/components/ui/` directory. You then own the code and can modify it freely.

The library covers three categories:
- **33 components** (Button, Chat, Code, EmojiPicker, Modal, PhoneInput, Stepper, Terminal, TreeView, …)
- **3 actions** (`active`, `shortcut`, `typewriter`) — Svelte 5 attachments/transitions
- **9 hooks** (`isMac`, `useAutoScroll`, `useBoolean`, `useClipboard`, `useFrecency`, `useMedia`, `usePromise`, `useRamp`, `useToc`)

## Project assumptions

The extras assume the project is already set up like a typical shadcn-svelte app:
- **Svelte 5** in runes mode (the library uses `$state`, `$bindable`, `$derived`, `Snippet`)
- **SvelteKit** (paths use `$lib/...`)
- **shadcn-svelte already initialized** — `npx shadcn-svelte init` has been run, `$lib/components/ui/` exists, Tailwind is configured, `app.css` has the shadcn theme tokens
- **bits-ui** installed (most components forward to bits-ui primitives)
- **Tailwind CSS** with the shadcn theme variables (`--background`, `--foreground`, `--primary`, etc.)

If any of these are missing, suggest initializing shadcn-svelte first: `npx shadcn-svelte init`. Do not attempt to install extras into a non-shadcn project — the styling will not resolve.

## When to use this skill

Trigger this skill whenever the user is in a Svelte/SvelteKit project and:

- Asks for a specific extra by name (e.g., "add a phone input", "use the EmojiPicker")
- Asks for a UI primitive that shadcn-svelte does **not** ship (shadcn-svelte has button, dialog, dropdown, etc., but **not** phone input, terminal, emoji picker, image cropper, NLP date input, etc. — those are extras)
- Wants to install a component from `@ieedan/shadcn-svelte-extras`
- Asks about `jsrepo`, `jsrepo init`, `jsrepo add`, or `jsrepo.config.ts`
- Wants to know the props or composition of an extras component
- Wants to copy-paste a component from the shadcn-svelte-extras website
- Asks how to do "X" (e.g., file upload, image cropping, language switching, theme toggle, multi-step form, star rating) in a SvelteKit app — reach for an extra if one exists

**Do not** trigger this skill for:
- React or plain HTML projects (extras are Svelte 5 only)
- Standard shadcn-svelte components (Button, Dialog, Select, etc. from `shadcn-svelte.com`) — those ship with `npx shadcn-svelte add`
- General Svelte questions unrelated to UI components

## Installation

### One-time: initialize jsrepo in the project

`jsrepo` is the recommended CLI. Run once per project:

```bash
npx jsrepo init @ieedan/shadcn-svelte-extras
```

This creates a `jsrepo.config.ts` that references the `@ieedan/shadcn-svelte-extras` registry. The user must configure the `paths` key so components, hooks, and utils land in the right directories (typically `$lib/components/ui/`, `$lib/hooks/`, `$lib/utils/`).

### Install a single component / action / hook

```bash
npx jsrepo add @ieedan/shadcn-svelte-extras/<slug>
```

Examples:
```bash
npx jsrepo add @ieedan/shadcn-svelte-extras/button
npx jsrepo add @ieedan/shadcn-svelte-extras/emoji-picker
npx jsrepo add @ieedan/shadcn-svelte-extras/use-clipboard
npx jsrepo add @ieedan/shadcn-svelte-extras/typewriter
```

### Alternative: shadcn-svelte CLI

Per-component, without initializing jsrepo:

```bash
npx shadcn-svelte add https://shadcn-svelte-extras.com/r/<slug>.json
```

Example: `npx shadcn-svelte add https://shadcn-svelte-extras.com/r/button.json`

### Optional: jsrepo MCP for agent workflows

If the user is doing agent-driven work and wants the agent to be able to search the registry and view component source, set up the jsrepo MCP:

```bash
npx jsrepo config mcp
```

This gives the agent capabilities like searching the registry, viewing source, viewing demos, and reading documentation programmatically.

## Conventions

### Imports

Extras use the namespaced import pattern, matching shadcn-svelte:

```svelte
<script lang="ts">
  import * as Button from "$lib/components/ui/button";
  import * as Modal from "$lib/components/ui/modal";
  import * as PhoneInput from "$lib/components/ui/phone-input";
</script>
```

You then use the namespace in markup: `<Button.Root>`, `<Modal.Content>`, `<PhoneInput />`, etc.

A few components are flat (no namespace) and are imported as a single named component: `<Snippet />`, `<GithubButton />`, `<Meter />`, `<Link />`, `<LanguageSwitcher />`, `<LightSwitch />`, `<ThemeSelector />`, `<NlpDateInput />`, `<Ipv4addressInput />`, `<PhoneInput />`, `<TagsInput />`, `<PmCommand />`, `<Toc />`.

### Composition (namespaced sub-components)

Most extras follow the `Foo.Root` / `Foo.Child` composition pattern. The composition tree for each component is documented in its reference page. Examples:

```text
Modal.Root
├── Modal.Trigger
└── Modal.Content
    ├── Modal.Header
    │   ├── Modal.Title
    │   └── Modal.Description
    └── Modal.Footer

NumberField.Root
└── NumberField.Group
    ├── NumberField.Decrement
    ├── NumberField.Input
    └── NumberField.Increment
```

When suggesting markup, always build the full composition tree — omitting required children (e.g., `Modal.Content` without `Modal.Header`) usually produces broken UI.

### $bindable props (Svelte 5 runes)

In the API tables, props marked `$bindable` are Svelte 5 bindable state — the caller can use `bind:` on them. Examples: `bind:value`, `bind:open`, `bind:step`, `bind:src`, `bind:hidden`, `bind:mode`, `bind:valid`.

Example:
```svelte
<NumberField.Root bind:value={count}>
  <NumberField.Group>
    <NumberField.Decrement />
    <NumberField.Input />
    <NumberField.Increment />
  </NumberField.Group>
</NumberField.Root>
```

### Snippet children (replaces Svelte 4 slots)

In the API tables, `Snippet` is the Svelte 5 children type. Pass children either as nested content or as the `children` prop:

```svelte
<Button.Root>Save</Button.Root>

<!-- equivalent -->
<Button.Root>{@render children()}</Button.Root>
```

For icon snippets, pass a snippet that renders the icon: `icon={() => <LucideCheck />}` or use the `{#snippet icon()}` block.

### bits-ui forwarding

Many extras forward props and behavior to bits-ui primitives. When the API table says "Documentation for this component's props can be found at bits-ui ..." and lists no props of its own, the component is a thin styled wrapper — the user should consult the linked bits-ui docs for the full prop surface.

Common bits-ui dependencies:
- `Modal.Root` / `Modal.Trigger` / `Modal.Content` / `Modal.Title` / `Modal.Description` → bits-ui Dialog
- `UnderlineTabs.Root` / `List` / `Trigger` / `Content` → bits-ui Tabs
- `Password.ToggleVisibility` → bits-ui Toggle
- `Meter` → bits-ui Meter
- `EmojiPicker.Search` → bits-ui Command.Input
- `StarRating.Root` → bits-ui RatingGroup
- `Chat.BubbleAvatar*` → bits-ui Avatar

### mode-watcher (theme components)

`LightSwitch` and `ThemeSelector` toggle the theme but need a watcher to actually apply it app-wide. Tell the user to include `<ModeWatcher />` from `mode-watcher` in their root `+layout.svelte`:

```svelte
<!-- +layout.svelte -->
<script>
  import { ModeWatcher } from "mode-watcher";
</script>

<ModeWatcher />
<slot />
```

### Superforms integration

`FileDropZone` integrates with [Superforms](https://superforms.rocks/) for file uploads. See the [Superforms file uploads guide](https://superforms.rocks/concepts/files#file-uploads) when the user is wiring up file uploads in a form.

### Paraglide integration

`LanguageSwitcher` integrates with [ParaglideJS](https://inlang.com/m/gerre34r/library-inlang-paraglideJs/sveltekit) for i18n. The docs page has a Paraglide example.

## Catalog

### Components (33)

| Name | Slug | One-liner |
| --- | --- | --- |
| Button | `button` | Extended button with `loading` and `onClickPromise` props |
| Chat | `chat` | Composable chat bubbles (`Chat.List`, `Chat.Bubble`, `Chat.BubbleMessage`, `Chat.BubbleAvatar*`) |
| Code | `code` | Shiki-powered code block with copy button, line numbers, line highlighting, overflow |
| Confirm Delete Dialog | `confirm-delete-dialog` | Confirm-delete dialog with optional text-match confirmation and Shift+Click skip |
| Copy Button | `copy-button` | Button that copies text and shows feedback animation |
| Emoji Picker | `emoji-picker` | Composable emoji picker with search, skin tone, recents, footer |
| Field Set | `field-set` | Form field grouping: `Root`, `Title`, `Content`, `Footer` (supports `destructive` variant) |
| File Drop Zone | `file-drop-zone` | Drag-drop / paste / click file uploads with `maxFiles`, `maxFileSize`, `accept`, reject handler |
| GitHub Button | `github-button` | Animated star count button linking to a GitHub repo |
| Image Cropper | `image-cropper` | Upload + crop images in a dialog using `svelte-easy-crop` |
| IPv4Address Input | `ipv4address-input` | Segmented IPv4 input with built-in validation |
| Language Switcher | `language-switcher` | Locale dropdown with Paraglide integration example |
| Light Switch | `light-switch` | Toggle theme (requires `ModeWatcher` in root layout) |
| Link | `link` | Styled anchor element |
| Meter | `meter` | Styled meter element (forwards to bits-ui Meter) |
| Modal | `modal` | Responsive dialog: dialog on desktop, drawer on mobile; supports `NestedRoot` for stacking |
| NLPDateInput | `nlp-date-input` | Natural-language date input with suggestions ("tomorrow at 5pm", "in 2 hours") |
| Number Field | `number-field` | Stepper with increment/decrement, ramp-on-hold, min/max |
| Password | `password` | Password input suite with visibility toggle, copy, and `zxcvbn-ts` strength meter |
| Phone Input | `phone-input` | International phone input with country selector and `valid` bindable |
| PMCommand | `pm-command` | Package-manager command tabs (npm/pnpm/yarn/bun) with copy; auto-completes commands via `package-manager-detector` |
| Rename | `rename` | Inline rename input with content-editable mode, text-area mode, external Edit/Save/Cancel controls |
| Snippet | `snippet` | Inline code snippet with copy button and multiline support |
| Split Button | `split-button` | Button + dropdown to pick which action runs (e.g., "Merge" / "Rebase") |
| Star Rating | `star-rating` | Star rating with half-stars, RTL support, custom colors/sizes (forwards to bits-ui RatingGroup) |
| Stepper | `stepper` | Multi-step indicator with vertical/horizontal orientation, `Next`/`Previous` buttons, multi-step form support |
| Tags Input | `tags-input` | Tag chips with validation, autocomplete suggestions, `restrictToSuggestions` mode |
| Terminal | `terminal` | macOS-style terminal with `TypingAnimation`, `AnimatedSpan`, `Loading`, `Loop` |
| Theme Selector | `theme-selector` | Light/dark/system dropdown (requires `ModeWatcher`) |
| Table of Contents | `toc` | Render a TOC from `Heading[]` produced by `useToc` |
| Tree View | `tree-view` | File-tree component with `Folder`/`File` and custom icons |
| Underline Tabs | `underline-tabs` | Horizontal tabs with animated underline (forwards to bits-ui Tabs) |
| Window | `window` | macOS-style styled window chrome wrapper |

### Actions (3)

| Name | Slug | One-liner |
| --- | --- | --- |
| Active | `active` | Svelte action that adds `data-active` to an `<a>` when its href matches the current route |
| Shortcut | `shortcut` | Svelte action for keyboard shortcuts (single or array); great for command palette hotkeys |
| Typewriter | `typewriter` | Svelte transition **and** Svelte 5 attachment (`attachTypewriter`) that reveals text char-by-char; supports `speed`, `delay`, `onComplete` |

### Hooks (9)

| Name | Slug | One-liner |
| --- | --- | --- |
| IsMac | `is-mac` | Reactive boolean for whether the user is on macOS; exports `cmdOrCtrl` and `optionOrAlt` for keyboard hints |
| UseAutoScroll | `use-auto-scroll` | Container that auto-scrolls to bottom when content grows (chat-style) |
| UseBoolean | `use-boolean` | Concise boolean state helper (replaces the `let x = $state(false); function toggle() { x = !x }` boilerplate) |
| UseClipboard | `use-clipboard` | Copy text with `copied` state, configurable reset delay (default 500ms), and `status` (success/failure) |
| UseFrecency | `use-frecency` | Track and sort items by frequency + recency of use (command-palette style) |
| UseMedia | `use-media` | Reactive Tailwind breakpoint (`sm`/`md`/`lg`/`xl`/`2xl`) or custom breakpoints |
| UsePromise | `use-promise` | Reactive promise state (`pending`/`fulfilled`/`rejected` + `value`/`error`) as a replacement for `{#await}` |
| UseRamp | `use-ramp` | Hold-to-repeat helper with accelerating schedule (for stepper buttons, number fields, etc.) |
| UseToc | `use-toc` | Generate `Heading[]` from page content for use with `Toc`; honors `data-toc-ignore` on headings or ancestors |

## Where to look for details

The SKILL.md you're reading is a router. For full prop tables, composition trees, and per-component notes:

- **`references/components.md`** — full reference for all 33 components (prop tables with type and default for every sub-component; composition trees; key gotchas)
- **`references/actions.md`** — full reference for the 3 Svelte actions (`active`, `shortcut`, `typewriter`)
- **`references/hooks.md`** — full reference for the 9 hooks (signatures, return shapes, usage patterns)

When the user asks about a specific component, read the matching section of the references file. Do not try to keep all 45 APIs (33 components + 3 actions + 9 hooks) in your head — look them up.

## Authoritative source

The canonical reference is <https://www.shadcn-svelte-extras.com/docs>. If anything here disagrees with the website, the website wins.

To pull the markdown for any docs page, append `.md` to the URL — e.g. <https://www.shadcn-svelte-extras.com/docs/components/button.md>. This is the recommended way to share docs with an agent: link to the `.md` URL rather than pasting the full page into a prompt.

The library is on GitHub at <https://github.com/ieedan/shadcn-svelte-extras>.

## Working with the user

When a user asks for "a phone input" or "an emoji picker" or similar:

1. **Identify the matching extra** from the catalog above. If multiple extras could fit (e.g., `LightSwitch` vs `ThemeSelector` for theming), briefly explain the difference and let the user choose.
2. **Check installation state.** If the project doesn't already use jsrepo, walk them through `npx jsrepo init @ieedan/shadcn-svelte-extras` once. Then install the specific component.
3. **Look up the full API** in the matching references file before writing markup — don't guess prop names.
4. **Show the full composition tree** in your suggested markup. Namespaced components need their required children.
5. **Mention cross-dependencies** when relevant: `mode-watcher` for theme components, Superforms for `FileDropZone` in forms, Paraglide for `LanguageSwitcher` i18n, bits-ui for any component whose API table forwards there.
6. **Prefer the user's existing tools.** If they already use `shadcn-svelte add`, use that. If they use `jsrepo`, use that. Don't force one over the other.

## Common gotchas

- **Empty `$lib/components/ui/` after install**: the user must run `npx shadcn-svelte init` first to set up the directory and utils. Extras expect `cn()` from `$lib/utils.ts`.
- **Theme toggle does nothing**: forgot `<ModeWatcher />` from `mode-watcher` in root layout.
- **`onClickPromise` button stuck loading**: the promise rejected. Either handle the rejection or wrap the handler in try/catch.
- **Modal renders as dialog on mobile**: that's by design. `Modal` is responsive — it uses Dialog on desktop and Drawer on mobile. If you need a plain Dialog, use shadcn-svelte's `Dialog`, not `Modal`.
- **`EmojiPicker` is huge**: the source includes the full emoji dataset (~10k lines). It only loads where imported — keep it out of route-level bundles if you don't need it everywhere.
- **`PhoneInput` and `IPv4Input` expose `valid` as `$bindable`**: bind it if you want to gate form submission on validity, rather than parsing the value yourself.
- **`Terminal` runs once by default**: wrap children in `<Terminal.Loop>` to make animations repeat.
- **`TagsInput` accepts arbitrary input by default**: set `restrictToSuggestions={true}` to lock to a fixed list, and provide a `validate` function for normalization (e.g., lowercase).

## What's NOT in extras

To avoid recommending extras when the user actually needs something else:

- **Standard primitives** (Button, Input, Label, Dialog, Select, Dropdown, Tabs, Tooltip, Toast, Avatar, Card, Accordion, …) ship with **shadcn-svelte** itself — `npx shadcn-svelte add button` (no `-extras`).
- **Forms / validation** — use `sveltekit-superforms` with `zod`. Extras play nicely with it but don't replace it.
- **Charts** — use `layerchart` or `svelte-chartjs`.
- **Tables** — use `tanstack-table` with shadcn-svelte's Table primitive.
- **Date pickers** — shadcn-svelte ships its own Calendar +Popover combo. `NLPDateInput` is for natural-language date entry, not a calendar replacement.
