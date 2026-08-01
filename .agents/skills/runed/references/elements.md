# Elements Utilities

This file documents the ten Elements-category utilities in Runed. Read it when the user wants to:
- Track the focused DOM element (activeElement)
- Get an element's full DOMRect or just width/height (ElementRect, ElementSize)
- Detect focus within a container (IsFocusWithin)
- Know if an element is in the viewport (IsInViewport, useIntersectionObserver)
- Track scroll position/direction/edges (ScrollState)
- Auto-grow a textarea (TextareaAutosize)
- Observe DOM mutations or element size changes (useMutationObserver, useResizeObserver)

## Table of Contents

- [activeElement / ActiveElement](#activeelement)
- [ElementRect](#elementrect)
- [ElementSize](#elementsize)
- [IsFocusWithin](#isfocuswithin)
- [IsInViewport](#isinviewport)
- [ScrollState](#scrollstate)
- [TextareaAutosize](#textareaautosize)
- [useIntersectionObserver](#useintersectionobserver)
- [useMutationObserver](#usemutationobserver)
- [useResizeObserver](#useresizeobserver)

---

## activeElement

Reactive access to the currently focused DOM element — pierces Shadow DOM boundaries. SSR-safe.

`activeElement` is a pre-instantiated singleton using the global document. `ActiveElement` is the class for scoping to a custom `DocumentOrShadowRoot` (e.g., inside an iframe or shadow root).

### Signature

```ts
import { activeElement, ActiveElement } from "runed";

interface ActiveElement {
  readonly current: Element | null;
}

// Class form for custom roots:
new ActiveElement({ document: DocumentOrShadowRoot });
```

### Example — Singleton

```svelte
<script lang="ts">
  import { activeElement } from "runed";
  let inputElement = $state<HTMLInputElement | undefined>();
</script>

<input bind:this={inputElement} />

{#if activeElement.current === inputElement}
  The input element is active!
{/if}
```

### Example — Outside a Component (Svelte Module)

```ts
// some-module.svelte.ts
import { activeElement } from "runed";

function logActiveElement() {
  $effect(() => {
    console.log("Active element is ", activeElement.current);
  });
}

logActiveElement();
```

### Gotchas

- **SSR-safe.** Returns `null` server-side. Use freely in SvelteKit without `IsMounted` gating.
- **Returns `null` when nothing is focused.** Always null-check before accessing properties.
- **Pierces Shadow DOM.** Walks through shadow roots to find the truly focused element, unlike `document.activeElement` which stops at shadow boundaries.

---

## ElementRect

Reactive access to an element's full `DOMRect` (dimensions + position), updating automatically on size/position changes.

### Signature

```ts
import { ElementRect } from "runed";

type Rect = Omit<DOMRect, "toJSON">;

interface ElementRectOptions {
  initialRect?: DOMRect;
}

class ElementRect {
  constructor(
    node: MaybeGetter<HTMLElement | undefined | null>,
    options?: ElementRectOptions
  );
  readonly current: Rect;
  readonly width: number;
  readonly height: number;
  readonly top: number;
  readonly left: number;
  readonly right: number;
  readonly bottom: number;
  readonly x: number;
  readonly y: number;
}
```

### Example

```svelte
<script lang="ts">
  import { ElementRect } from "runed";
  let el = $state<HTMLElement>();
  const rect = new ElementRect(() => el, {
    initialRect: new DOMRect(0, 0, 100, 50),
  });
</script>

<textarea bind:this={el}></textarea>
<p>Width: {rect.width} Height: {rect.height}</p>
<p>Top: {rect.top} Left: {rect.left}</p>
```

### Gotchas

- **First arg is a `MaybeGetter`** — pass the element directly or as a getter. A getter is preferred so the element ref can be late-bound via `bind:this`.
- **`initialRect` seeds the value before measurement.** Useful to avoid layout flashes (e.g., default to a sensible size during SSR or before mount).
- **Position fields (`top`, `left`, `right`, `bottom`, `x`, `y`) update on scroll and resize.** If you only need dimensions, prefer `ElementSize` — it's lighter.

---

## ElementSize

Reactive access to an element's width and height only. Lighter than `ElementRect` when you don't need position.

### Signature

```ts
import { ElementSize } from "runed";

interface ElementSize {
  readonly width: number;
  readonly height: number;
}

new ElementSize(getter: Getter<HTMLElement>);
```

### Example

```svelte
<script lang="ts">
  import { ElementSize } from "runed";
  let el = $state() as HTMLElement;
  const size = new ElementSize(() => el);
</script>

<textarea bind:this={el}></textarea>
<p>Width: {size.width} Height: {size.height}</p>
```

### Gotchas

- **Only exposes `width` and `height`** (no position). For position, use `ElementRect`.
- **Constructor takes a `Getter<HTMLElement>`** — pass `() => el`, not `el` directly.
- **Under the hood uses `ResizeObserver`.** Cleanup is automatic.

---

## IsFocusWithin

Reactively tracks whether any descendant element has focus within a specified container. Useful for forms, menus, dropdowns.

### Signature

```ts
import { IsFocusWithin } from "runed";

class IsFocusWithin {
  constructor(node: MaybeGetter<HTMLElement | undefined | null>);
  readonly current: boolean;
}
```

### Example

```svelte
<script lang="ts">
  import { IsFocusWithin } from "runed";
  let formElement = $state<HTMLFormElement>();
  const focusWithinForm = new IsFocusWithin(() => formElement);
</script>

<form bind:this={formElement}>
  <input type="text" />
  <button type="submit">Submit</button>
</form>

<p>Focus within form: {focusWithinForm.current}</p>
```

### Gotchas

- **First arg is a `MaybeGetter`.** Pass `() => el` for late binding.
- **Pierce behavior:** does NOT pierce Shadow DOM by default. If you need to detect focus in slotted or shadow content, scope an `ActiveElement` to that shadow root instead.

---

## IsInViewport

Reactive boolean indicating whether an element is currently visible in the viewport. Built on top of `useIntersectionObserver`.

### Signature

```ts
import { IsInViewport } from "runed";

export type IsInViewportOptions = UseIntersectionObserverOptions;

export declare class IsInViewport {
  constructor(
    node: MaybeGetter<HTMLElement | null | undefined>,
    options?: IsInViewportOptions
  );
  get current(): boolean;                          // viewport intersection state
  get observer(): UseIntersectionObserverReturn;   // underlying observer
}
```

Inherits `UseIntersectionObserverOptions`, notably `once: true` (auto-stop after first intersection) and `root`.

### Example — One-time Fade-In

```svelte
<script lang="ts">
  import { IsInViewport } from "runed";
  let targetNode = $state<HTMLElement>();
  const inViewport = new IsInViewport(() => targetNode, { once: true });
</script>

<p bind:this={targetNode} class:visible={inViewport.current}>Target node</p>
```

### Gotchas

- **`once: true` stops observing after first intersection.** Perfect for one-time animations (fade-in on scroll). For continuous visibility tracking, omit `once`.
- **Exposes the underlying `observer`** so you can call `pause()`, `resume()`, `stop()`, and read `isActive` directly. Don't destructure `observer.isActive` — access it as `inViewport.observer.isActive`.

---

## ScrollState

Reactive scroll tracking — position (`x` / `y`), direction, edge-arrival state, progress percentage, plus programmatic scrolling helpers. Inspired by VueUse's `useScroll`.

### Signature

```ts
import { ScrollState } from "runed";

new ScrollState(options: ScrollStateOptions);
```

Where `options.element` is required.

### Properties / Methods

| Member | Description |
| --- | --- |
| `x`, `y` | Reactive get/set scroll positions. Writing to them scrolls programmatically. |
| `directions` | Active scroll directions: `{ left, right, top, bottom }` booleans. |
| `arrived` | Edge-arrival booleans: `{ top, right, bottom, left }`. |
| `progress` | Percentage scrolled on x/y axis. |
| `scrollTo(x, y)` | Programmatic scroll. |
| `scrollToTop()` | Scroll to top. |
| `scrollToBottom()` | Scroll to bottom. |

### Configuration Options

| Option | Type | Description |
| --- | --- | --- |
| `element` | `MaybeGetter<HTMLElement \| Window \| Document \| null>` | The scroll container (required). |
| `idle` | `MaybeGetter<number \| undefined>` | Debounce (ms) after scroll ends. Default `200`. |
| `offset` | `{ top?, bottom?, left?, right? }` | Pixel thresholds for "arrived" state. Default `0` for all. |
| `onScroll` | `(e: Event) => void` | Callback for scroll events. |
| `onStop` | `(e: Event) => void` | Callback after scrolling stops (debounced by `idle`). |
| `eventListenerOptions` | `AddEventListenerOptions` | Scroll listener options. Default `{ passive: true, capture: false }`. |
| `behavior` | `ScrollBehavior` | `"auto"`, `"smooth"`, etc. Default `"auto"`. |
| `onError` | `(error: unknown) => void` | Optional error handler. Default `console.error`. |

### Example

```svelte
<script lang="ts">
  import { ScrollState } from "runed";
  let el = $state<HTMLElement>();
  const scroll = new ScrollState({
    element: () => el,
    offset: { top: 32, bottom: 32 },
    onStop: () => console.log("user stopped scrolling"),
  });
</script>

<div bind:this={el} style="overflow: auto; height: 200px;">
  <!-- long content -->
</div>

<p>Position: {scroll.x}, {scroll.y}</p>
<p>At top: {scroll.arrived.top} | At bottom: {scroll.arrived.bottom}</p>
<button onclick={() => scroll.scrollToBottom()}>Jump to bottom</button>
```

### Gotchas

- **`element` accepts `Window`, `Document`, or `HTMLElement`.** Pass `() => window` for whole-window scroll tracking.
- **Writing to `scroll.x` / `scroll.y` programmatically scrolls the element.** This is bidirectional reactivity — both reads and writes work.
- **`onStop` is debounced by `idle` ms** after scrolling ends. Default `200ms` — adjust if you need faster or slower stop detection.
- **Layout direction is respected.** Edge-arrival state accounts for RTL/flex-reverse layouts when computing "arrived at top/bottom".

---

## TextareaAutosize

Auto-grow/shrink a `<textarea>` to fit its content without layout shifts. Mirrors the textarea off-screen and measures its scroll height.

### Signature

```ts
import { TextareaAutosize } from "runed";

new TextareaAutosize(options: {
  element: Getter<HTMLElement | undefined>;        // required
  input: Getter<string>;                            // required
  onResize?: () => void;
  styleProp?: "height" | "minHeight";               // default "height"
  maxHeight?: number;                                // pixels
});
```

### Configuration Options

| Option | Type | Description |
| --- | --- | --- |
| `element` | `Getter<HTMLElement \| undefined>` | The target textarea (required). |
| `input` | `Getter<string>` | Reactive input value (required). |
| `onResize` | `() => void` | Called whenever the height is updated. |
| `styleProp` | `"height" \| "minHeight"` | CSS property: `"height"` resizes both ways; `"minHeight"` grows only. Default `"height"`. |
| `maxHeight` | `number` | Maximum height in pixels before scroll appears. Default unlimited. |

### Example

```svelte
<script lang="ts">
  import { TextareaAutosize } from "runed";
  let el = $state<HTMLTextAreaElement>(null!);
  let value = $state("");
  new TextareaAutosize({ element: () => el, input: () => value });
</script>

<textarea bind:this={el} bind:value></textarea>
```

### Gotchas

- **`element` and `input` are `Getter`, not `MaybeGetter`.** Both must be functions: `() => el` and `() => value`. Passing the value directly will fail to react.
- **`styleProp: "minHeight"` only grows.** If you want the textarea to shrink when content is removed, use the default `"height"`.
- **Recalculates on content/element/width changes.** If you change the textarea's CSS (padding, font-size), the autosize will recompute on the next input event.
- **Internally clones the textarea off-screen** and copies computed styles for accurate measurement. Don't be alarmed if you see a hidden clone in devtools.

---

## useIntersectionObserver

Reactive wrapper around the native `IntersectionObserver`. Observe when a target element enters/leaves a (root) container's viewport.

For a simpler boolean API, use `IsInViewport` (which is built on this).

### Signature

```ts
import { useIntersectionObserver } from "runed";

useIntersectionObserver(
  target: Getter<HTMLElement | null>,
  callback: (entries: IntersectionObserverEntry[]) => void,
  options?: UseIntersectionObserverOptions
): {
  pause: () => void;
  resume: () => void;
  stop: () => void;
  isActive: boolean;        // getter — DO NOT DESTRUCTURE
};
```

### Configuration Options

| Option | Type | Description |
| --- | --- | --- |
| `root` | `Getter<HTMLElement \| null>` | The container viewport (default: browser viewport). |
| `rootMargin` | `string` | Margin around the root. Standard CSS margin string. |
| `threshold` | `number \| number[]` | Intersection ratio(s) at which to fire. `0` = any pixel visible, `1` = fully visible. |
| `once` | `boolean` | Auto-stop after first intersection. |

### Example

```svelte
<script lang="ts">
  import { useIntersectionObserver } from "runed";
  let target = $state<HTMLElement | null>(null);
  let root = $state<HTMLElement | null>(null);
  let isIntersecting = $state(false);

  const observer = useIntersectionObserver(
    () => target,
    (entries) => {
      const entry = entries[0];
      if (entry) isIntersecting = entry.isIntersecting;
    },
    { root: () => root, threshold: 0.5 }
  );
</script>

<div bind:this={root} style="height: 200px; overflow: auto;">
  <div bind:this={target}>Am I visible?</div>
</div>
<button onclick={observer.pause}>Pause</button>
<button onclick={observer.resume}>Resume</button>
```

### Gotchas

- **`isActive` is a getter — do not destructure.** `const { isActive } = observer` ❌. Always access as `observer.isActive`. Same caution applies to `IsInViewport`'s exposed `.observer`.
- **`target` and `root` are `Getter`.** Pass `() => el`, not `el`. Allows late binding via `bind:this`.
- **`once: true` stops after first intersection.** Useful for one-time animations; for ongoing visibility tracking, omit it.
- **The callback receives an array of entries** (one per observed target). For single-target observation, read `entries[0]`.

---

## useMutationObserver

Reactive wrapper around `MutationObserver`. Observe DOM changes (attributes, childList, subtree, characterData) on a target element.

### Signature

```ts
import { useMutationObserver } from "runed";

useMutationObserver(
  target: Getter<HTMLElement | null>,
  callback: (mutations: MutationRecord[]) => void,
  options?: MutationObserverInit
): { stop: () => void };
```

Returns only `{ stop }` — no `pause`/`resume`/`isActive`.

### Configuration Options (standard `MutationObserverInit`)

| Option | Description |
| --- | --- |
| `attributes` | Watch attribute changes. |
| `attributeFilter` | Array of attribute names to filter. |
| `attributeOldValue` | Include old attribute values in records. |
| `childList` | Watch direct child additions/removals. |
| `subtree` | Watch all descendants. |
| `characterData` | Watch text content changes. |
| `characterDataOldValue` | Include old text in records. |

### Example

```svelte
<script lang="ts">
  import { useMutationObserver } from "runed";
  let el = $state<HTMLElement | null>(null);
  const messages = $state<string[]>([]);

  useMutationObserver(
    () => el,
    (mutations) => {
      const mutation = mutations[0];
      if (mutation) messages.push(mutation.attributeName!);
    },
    { attributes: true }
  );
</script>
```

### Gotchas

- **Returns only `{ stop }`.** No `pause` / `resume` / `isActive` — call `stop()` to permanently stop observing.
- **`target` is a `Getter`.** Pass `() => el` for late binding.
- **Pick the right options** — observing `subtree: true` with `childList: true` can be expensive on large DOMs. Be specific (e.g., `attributes: true, attributeFilter: ["class", "data-state"]`).

---

## useResizeObserver

Reactive wrapper around `ResizeObserver`. Detect changes in an element's size. For a simpler declarative API, use `ElementSize` or `ElementRect`.

### Signature

```ts
import { useResizeObserver } from "runed";

useResizeObserver(
  target: Getter<HTMLElement | null>,
  callback: (entries: ResizeObserverEntry[]) => void,
  options?: ResizeObserverOptions
): { stop: () => void };
```

Returns only `{ stop }`.

### Configuration Options (`ResizeObserverOptions`)

Standard `ResizeObserverOptions`:

| Option | Description |
| --- | --- |
| `box` | Which box model to observe: `"content-box"` (default), `"border-box"`, or `"device-pixel-content-box"`. |

### Example

```svelte
<script lang="ts">
  import { useResizeObserver } from "runed";
  let el = $state<HTMLElement | null>(null);
  let text = $state("");

  useResizeObserver(
    () => el,
    (entries) => {
      const entry = entries[0];
      if (entry) {
        const { width, height } = entry.contentRect;
        text = `width: ${width};\nheight: ${height};`;
      }
    }
  );
</script>

<div bind:this={el}>Resize me</div>
<pre>{text}</pre>
```

### Gotchas

- **Returns only `{ stop }`.** No `pause` / `resume` / `isActive`.
- **Read dimensions off `entry.contentRect`** in the callback (not the entry itself).
- **For declarative reactive state, prefer `ElementSize` or `ElementRect`.** They wrap this observer and expose reactive `.width` / `.height` / `.current` properties.
- **`target` is a `Getter`.** Pass `() => el` for late binding.
