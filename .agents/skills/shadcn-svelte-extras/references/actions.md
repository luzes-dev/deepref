# shadcn-svelte-extras — Actions Reference

Svelte 5 actions and attachments shipped with shadcn-svelte-extras. All three are installed individually via `jsrepo add`.

## Table of Contents

- [Active](#active)
- [Shortcut](#shortcut)
- [Typewriter](#typewriter)

---

## Active

A Svelte action that determines if a link is active. Adds a `data-active` attribute to the `<a>` tag so you can style active links.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/active`

**Usage as an action:**

```svelte
<script lang="ts">
  import { active } from "$lib/actions/active";
</script>

<a href="/about" use:active>About</a>
```

**Usage as an attachment (Svelte 5):**

For Svelte 5 attachments, the action is also exported as an attachment. Attach it via the `attach:` syntax (or whichever attachment wrapper your project uses).

**Styling:** Use `[data-active]` in your CSS or Tailwind:

```css
a[data-active] {
  font-weight: 600;
  color: hsl(var(--primary));
}
```

```svelte
<a href="/about" use:active class="data-[active]:font-semibold data-[active]:text-primary">About</a>
```

**Behavior notes:**
- Active is determined by matching the link's `href` against the current route.
- Works with both static and dynamic routes.
- For nested navigation (e.g., a parent link that should be active when any child route is active), the matching logic is prefix-based — make sure the parent's `href` matches the start of the child paths.

---

## Shortcut

A Svelte action to create keyboard shortcuts for your application. Supports single shortcuts or arrays of shortcuts.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/shortcut`

**Usage as an action:**

```svelte
<script lang="ts">
  import { shortcut } from "$lib/actions/shortcut";

  function handleSubmit() {
    // ...
  }
</script>

<!-- Single shortcut -->
<svelte:window use:shortcut={{ shortcut: { key: "Enter", mod: true }, callback: handleSubmit }} />

<!-- Multiple shortcuts -->
<svelte:window use:shortcut={[
  { shortcut: { key: "1", mod: true }, callback: () => gotoTab(1) },
  { shortcut: { key: "2", mod: true }, callback: () => gotoTab(2) },
  { shortcut: { key: "k", mod: true }, callback: () => openCommandPalette() },
]} />
```

**Usage as an attachment (Svelte 5):**

For Svelte 5 attachments, use the exported attachment variant.

**Behavior notes:**
- `mod` matches `Cmd` on macOS and `Ctrl` on other platforms — use this for cross-platform shortcuts. Combine with the [`isMac`](hooks.md#ismac) hook if you need to display the actual key (⌘ vs Ctrl) to the user.
- Shortcut keys are case-insensitive.
- The action handles `keydown` events and prevents default browser behavior when the shortcut matches.
- For shortcuts that should only fire when an input is focused (or not focused), wrap the callback with your own focus checks.

---

## Typewriter

A Svelte transition **and** Svelte 5 attachment that reveals text character by character. Useful for hero sections, landing pages, and demo terminals.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/typewriter`

### Usage as a transition

Use as a transition on an element whose full text is already in the DOM. The transition progressively reveals it.

```svelte
<script lang="ts">
  import { typewriter } from "$lib/actions/typewriter";
</script>

<p in:typewriter={{ speed: 1.5, delay: 200 }}>Hello, world!</p>
```

### Usage as an attachment (Svelte 5)

For Svelte 5 attachments, use `attachTypewriter` so the effect applies when the node is mounted.

```svelte
<script lang="ts">
  import { attachTypewriter } from "$lib/actions/typewriter";
</script>

<p use:attachTypewriter={{ speed: 1.5, delay: 200, onComplete: () => console.log("done") }}>
  Hello, world!
</p>
```

### Options

| Option | Type | Description |
| --- | --- | --- |
| `speed` | `number` | Higher = faster. Controls characters per tick. |
| `delay` | `number` | Milliseconds before starting. |
| `onComplete` | `() => void` | Called when the full string is visible. |

**Behavior notes:**
- The text must be in the DOM before the transition/attachment runs — Typewriter reveals existing text, it does not source text from a prop.
- For looping animations, combine with a state flag and a keyed `{#if}` block to re-trigger the transition.
- For the terminal demo pattern (typing animation that runs once and stops), use [`Terminal.TypingAnimation`](components.md#terminal) instead — it's a higher-level wrapper.
