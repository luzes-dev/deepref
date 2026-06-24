# shadcn-svelte-extras — Hooks Reference

Svelte 5 hooks shipped with shadcn-svelte-extras. All nine are installed individually via `jsrepo add`. Each hook lives in `$lib/hooks/` and is a `.svelte.ts` file (runes-aware).

## Table of Contents

- [IsMac](#ismac)
- [UseAutoScroll](#useautoscroll)
- [UseBoolean](#useboolean)
- [UseClipboard](#useclipboard)
- [UseFrecency](#usefrecency)
- [UseMedia](#usemedia)
- [UsePromise](#usepromise)
- [UseRamp](#useramp)
- [UseToc](#usetoc)

---

## IsMac

A hook to determine if the user is on a Mac. Acknowledges inspiration from the shadcn-svelte [`useIsMac`](https://github.com/huntabyte/shadcn-svelte/blob/main/docs/src/lib/hooks/use-is-mac.svelte.ts) hook, with enhancements by [Thomas G. Lopes](https://github.com/tglide).

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/is-mac`

**Usage:**

```svelte
<script lang="ts">
  import { isMac, cmdOrCtrl, optionOrAlt } from "$lib/hooks/is-mac.svelte";
</script>

{#if isMac}
  Press <kbd>{cmdOrCtrl}</kbd> + K
{:else}
  Press <kbd>{cmdOrCtrl}</kbd> + K
{/if}
```

**Exports:**
- `isMac` — reactive boolean (`true` when the user is on macOS).
- `cmdOrCtrl` — reactive string: `"⌘"` on macOS, `"Ctrl"` elsewhere.
- `optionOrAlt` — reactive string: `"⌥"` on macOS, `"Alt"` elsewhere.

**Behavior notes:**
- The detection is based on `navigator.userAgent` and is therefore client-side only. In SSR, `isMac` is `false` until hydration.
- Use this to display platform-appropriate keyboard hints in your UI without writing the platform check yourself.
- For shortcuts themselves, use the [`shortcut`](actions.md#shortcut) action — it handles the `mod` semantics internally.

---

## UseAutoScroll

A hook to enable the creation of containers that automatically scroll to the bottom of their content. Perfect for chat UIs, log viewers, and other append-only lists.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/use-auto-scroll`

**Usage:**

```svelte
<script lang="ts">
  import { useAutoScroll } from "$lib/hooks/use-auto-scroll.svelte";

  const { snapshot, viewport, scrollToBottom } = useAutoScroll();
</script>

<div bind:this={viewport}>
  {#each messages as message}
    <div>{message.text}</div>
  {/each}
  <!-- snapshot anchor at the end -->
  <div bind:this={snapshot}></div>
</div>

<button onclick={scrollToBottom}>Jump to latest</button>
```

**Behavior notes:**
- The hook tracks whether the user has scrolled away from the bottom. If they have, auto-scroll is paused — appending new content does not yank them down. When they scroll back to the bottom, auto-scroll resumes.
- This solves the "auto-scroll stole my scroll position" problem in chat UIs.
- Pair with [`Chat`](components.md#chat) for chat-style message lists.

---

## UseBoolean

A hook to simplify working with boolean values — replaces the `let x = $state(false); function toggle() { x = !x }` boilerplate.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/use-boolean`

Acknowledges inspiration from [`strlrd-29/hookcn`](https://hookcn.ouassim.tech/docs/hooks/use-boolean).

**Usage — Before:**

```svelte
<script lang="ts">
  let open = $state(false);
  function toggle() { open = !open; }
  function open_() { open = true; }
  function close() { open = false; }
</script>
```

**Usage — After:**

```svelte
<script lang="ts">
  import { useBoolean } from "$lib/hooks/use-boolean.svelte";

  const { state: open, toggle, setTrue: open_, setFalse: close } = useBoolean(false);
</script>
```

**Returns:**
- `state` — the current boolean value (reactive).
- `toggle()` — flips the value.
- `setTrue()` / `setFalse()` — set to a specific value.

**Behavior notes:**
- Accepts an initial value (default `false`).
- The returned `state` is a Svelte 5 `$state` rune — bind directly to it or read it in derived state.

---

## UseClipboard

A hook to simplify copying text to the clipboard. Tracks `copied` state and `status` (success/failure).

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/use-clipboard`

**Usage:**

```svelte
<script lang="ts">
  import { useClipboard } from "$lib/hooks/use-clipboard.svelte";

  const { copy, copied, status } = useClipboard();
</script>

<button onclick={() => copy("Hello, world!")}>
  {#if copied}
    Copied!
  {:else}
    Copy
  {/if}
</button>
```

**Delay:**

`UseClipboard` delays resetting `.copied` to `false` so you can show a status to your users. Default delay: `500ms`.

```svelte
<script>
  const { copy, copied } = useClipboard({ delay: 1500 }); // show "Copied!" for 1.5s
</script>
```

**Status:**

Check `.status` to determine if the copy was a success or failure and update the UI accordingly.

```svelte
<script>
  const { copy, status } = useClipboard();
</script>

<button onclick={() => copy(text)}>
  {#if status === "success"}
    Copied!
  {:else if status === "error"}
    Failed — copy manually
  {:else}
    Copy
  {/if}
</button>
```

**Returns:**
- `copy(text)` — async function that writes to the clipboard.
- `copied` — reactive boolean, `true` for `delay` ms after a successful copy.
- `status` — `"idle"` | `"success"` | `"error"`.

**Behavior notes:**
- Clipboard write requires a user gesture (a click or key press). Calling `copy()` outside a user-initiated event will reject.
- On HTTP (non-HTTPS) origins, the clipboard API may be unavailable — `status` will be `"error"` in that case.

---

## UseFrecency

A hook to track and sort items based on their frequency of use (frequency + recency). The classic command-palette ranking algorithm.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/use-frecency`

**Usage:**

```svelte
<script lang="ts">
  import { useFrecency } from "$lib/hooks/use-frecency.svelte";

  const items = ["Angular", "Svelte", "React", "Vue"];
  const { sorted, touch } = useFrecency(items);

  function selectItem(item: string) {
    touch(item); // bump this item's frecency
    // ... do something with item
  }
</script>

{#each sorted as item}
  <button onclick={() => selectItem(item)}>{item}</button>
{/each}
```

**Returns:**
- `sorted` — reactive array of items, sorted by frecency (most-frequently-and-recently-used first).
- `touch(item)` — record a use of `item`. Calling this on selection is what makes the sort adaptive.

**Behavior notes:**
- Frecency = frequency × recency decay. Items used often **and** recently rank highest.
- Perfect for command palettes, recent-files lists, and quick-switchers.
- The hook is in-memory by default. For persistence, wrap `touch` and the initial items list with `runed`'s `PersistedState` or your own `localStorage` adapter.

---

## UseMedia

A hook to track the size of the screen using the standard Tailwind CSS breakpoints (`sm`, `md`, `lg`, `xl`, `2xl`).

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/use-media`

**Usage:**

```svelte
<script lang="ts">
  import { useMedia } from "$lib/hooks/use-media.svelte";

  const { breakpoint } = useMedia();
</script>

<p>Current breakpoint: {breakpoint}</p>

{#if breakpoint === "2xl"}
  <BigLayout />
{:else if breakpoint === "sm" || breakpoint === "xs"}
  <CompactLayout />
{:else}
  <DefaultLayout />
{/if}
```

**Custom breakpoints:**

You can also define your own breakpoints and get full type safety.

```svelte
<script lang="ts">
  import { useMedia } from "$lib/hooks/use-media.svelte";

  const { breakpoint } = useMedia({
    custom: 500,
  });
</script>

<!-- breakpoint will be "custom" when viewport width >= 500px -->
```

**Returns:**
- `breakpoint` — reactive string, the name of the largest currently-matching breakpoint.

**Behavior notes:**
- The default breakpoints match Tailwind's: `sm` (640px), `md` (768px), `lg` (1024px), `xl` (1280px), `2xl` (1536px). Below `sm`, the hook returns `"xs"`.
- Listens to `resize` and `matchMedia` events; updates reactively.
- Use this to drive layout decisions in JavaScript that can't be expressed purely in CSS.

---

## UsePromise

A hook to manage the state of a promise reactively in the absence of `{#await}`.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/use-promise`

**Usage:**

```svelte
<script lang="ts">
  import { usePromise } from "$lib/hooks/use-promise.svelte";

  // Set a default value for the version until it is streamed back from the server.
  const { state, value, error } = usePromise(fetchVersion(), { defaultValue: "1.0.0" });
</script>

{#if state === "pending"}
  Loading...
{:else if state === "fulfilled"}
  Version: {value}
{:else if state === "rejected"}
  Error: {error?.message}
{/if}
```

**Returns:**
- `state` — `"pending"` | `"fulfilled"` | `"rejected"` (reactive).
- `value` — the resolved value (or `defaultValue` while pending).
- `error` — the rejection reason (or `null` while pending/fulfilled).

**Behavior notes:**
- Useful when you need the resolved value in JS (not just in markup) — `{#await}` only helps in markup.
- Pass `defaultValue` to render meaningful UI during the pending state (e.g., a default version string while the real version streams from the server).
- The hook tracks the latest call — if you reassign the promise, the hook re-subscribes.

---

## UseRamp

Repeatedly call a function on an accelerating schedule while a condition holds. Powers the "hold-to-repeat, ramping counter" behavior of [`NumberField`](components.md#number-field)'s increment/decrement buttons.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/use-ramp`

**Usage:**

```svelte
<script lang="ts">
  import { useRamp } from "$lib/hooks/use-ramp.svelte";

  const { start, reset } = useRamp({
    callback: () => count++,
    canRamp: () => count < max,
    // optional — override the defaults:
    startDelay: 400,
    rampUpTime: 1500,
    minFrequency: 35,
    maxFrequency: 200,
  });
</script>

<button onpointerdown={start} onpointerup={reset} onpointerleave={reset}>
  Hold to increment
</button>
```

**Returns:**
- `start()` — begins the ramp. Call on `pointerdown`.
- `reset()` — stops the ramp and resets the schedule. Call on `pointerup` / `pointerleave` / `blur`.

**Options:**
- `callback()` — called on each ramp tick.
- `canRamp()` — gate function; if it returns `false`, the ramp pauses. Call `reset()` inside the ramp loop when `canRamp` becomes `false` (e.g., when `value` hits `max`).
- `startDelay` — ms before the first repeat (default `400`).
- `rampUpTime` — ms over which to accelerate from `minFrequency` to `maxFrequency` (default `0`).
- `minFrequency` — slowest tick period in ms (default `35`).
- `maxFrequency` — fastest tick period in ms (default `35`).

**Behavior notes:**
- Use for any "hold to repeat" interaction: stepper buttons, slider nudging, zoom controls, etc.
- The schedule accelerates from `minFrequency` to `maxFrequency` over `rampUpTime`. If both frequencies are equal (the default), the ramp has no acceleration.
- Always call `reset()` on release — leaving the ramp running will keep ticking forever.

---

## UseToc

A hook to generate a table of contents based on the page content. Pairs with the [`Toc`](components.md#table-of-contents-toc) component.

**Install:** `npx jsrepo add @ieedan/shadcn-svelte-extras/use-toc`

**Usage:**

```svelte
<script lang="ts">
  import { useToc } from "$lib/hooks/use-toc.svelte";
  import Toc from "$lib/components/ui/toc";

  const { toc } = useToc();
</script>

<Toc {toc} />

<main>
  <h1>Page Title</h1>
  <h2>Section 1</h2>
  <p>...</p>
  <h2>Section 2</h2>
  <p>...</p>
  <h3>Subsection</h3>
  <p>...</p>
</main>
```

**Ignoring headings:**

Exclude headings from the TOC by adding the `data-toc-ignore` attribute to the heading itself or to any parent element containing the heading.

```svelte
<!-- This heading will not appear in the TOC -->
<h2 data-toc-ignore>Skip me</h2>

<!-- None of these headings will appear in the TOC -->
<div data-toc-ignore>
  <h2>Skip us</h2>
  <h3>Also skip</h3>
</div>
```

**Returns:**
- `toc` — reactive `Heading[]` where each `Heading` is roughly `{ id: string; level: number; text: string; children?: Heading[] }`.

**Behavior notes:**
- The hook scans the DOM for headings (`h1`–`h6`) within a target root (default: the document body).
- Headings get auto-assigned IDs (based on text content) so the TOC can deep-link to them.
- The hook re-scans when content changes (MutationObserver).
- For docs-style sites with markdown rendering, this is the standard TOC pattern — combine with [`Toc`](components.md#table-of-contents-toc) for rendering.
