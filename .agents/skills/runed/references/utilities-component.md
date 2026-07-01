# Animation, Utilities & Component

This file documents the seven utilities in the Animation, Utilities, and Component categories. Read it when the user wants to:
- Run code on every animation frame with optional FPS cap (AnimationFrames)
- Render proper HTML boolean attributes (boolAttr)
- Register cleanup in an effect context (onCleanup)
- Debounce or throttle a callback function (useDebounce, useThrottle)
- Run code on an interval with pause/resume/counter (useInterval)
- Know if a component has mounted (IsMounted)

## Table of Contents

- [AnimationFrames (Animation)](#animationframes)
- [boolAttr (Utilities)](#boolattr)
- [onCleanup (Utilities)](#oncleanup)
- [useDebounce (Utilities)](#usedebounce)
- [useInterval (Utilities)](#useinterval)
- [useThrottle (Utilities)](#usethrottle)
- [IsMounted (Component)](#ismounted)

---

## AnimationFrames

Declarative wrapper around `requestAnimationFrame` with FPS-limiting and frame metrics (FPS, delta), handling cleanup automatically.

### Signature

```ts
import { AnimationFrames } from "runed";

new AnimationFrames(
  callback: (args: { delta: number }) => void,
  options?: { fpsLimit?: MaybeGetter<number> }
);
```

`delta` is the milliseconds since the previous frame.

### Properties / Methods

| Member | Description |
| --- | --- |
| `fps` | Current frames-per-second (read-only). |
| `running` | Boolean; toggleable to start/stop. |
| `fpsLimit` (option) | Target FPS cap; `0` means uncapped. `MaybeGetter` — can be reactive. |

### Example — Counter with FPS Display

```svelte
<script lang="ts">
  import { AnimationFrames } from "runed";
  let frames = $state(0);
  let fpsLimit = $state(10);
  let delta = $state(0);

  const animation = new AnimationFrames(
    (args) => { frames++; delta = args.delta; },
    { fpsLimit: () => fpsLimit }
  );
</script>

<pre>
Frames: {frames}
FPS: {animation.fps.toFixed(0)}
Delta: {delta.toFixed(0)}ms
</pre>

<button onclick={() => (animation.running = !animation.running)}>
  {animation.running ? "Stop" : "Start"}
</button>
```

### Example — Uncapped Loop

```svelte
<script lang="ts">
  import { AnimationFrames } from "runed";
  let angle = $state(0);
  new AnimationFrames(({ delta }) => {
    angle = (angle + delta * 0.1) % 360;   // 0.1 deg per ms = 100 deg/sec
  }, { fpsLimit: 0 });   // uncapped
</script>

<div style="transform: rotate({angle}deg);">spinning</div>
```

### Gotchas

- **`fpsLimit` is a `MaybeGetter`.** Use a getter for reactive FPS limits: `() => userFpsPref`.
- **`fpsLimit = 0` disables limiting.** Use it when you want every animation frame the browser gives you (typically 60 Hz, 120 Hz on high-refresh displays).
- **Toggle `animation.running` to start/stop the loop.** Don't call `requestAnimationFrame` yourself — let Runed manage the loop.
- **Cleanup is automatic on component destruction.** No need to call `cancelAnimationFrame`.
- **`delta` is in milliseconds.** Multiply by your speed factor to get frame-rate-independent motion (`delta * 0.1` = 0.1 units per ms).

---

## boolAttr

Converts any value into `""` (empty string) or `undefined` for proper HTML boolean-attribute behavior.

Svelte has a gotcha: `data-active={false}` renders as `data-active="false"` — the attribute is still present, breaking CSS `[data-active]` selectors. `boolAttr` returns `""` when truthy (so the attribute renders) and `undefined` when falsy (so Svelte omits the attribute entirely).

### Signature

```ts
import { boolAttr } from "runed";

function boolAttr(value: unknown): "" | undefined;
```

### Returns

| Input | Output | HTML Result |
| --- | --- | --- |
| Truthy (`true`, `1`, `"x"`, `{}`, `[]`) | `""` | Attribute is present (`<div data-active>`) |
| Falsy (`false`, `0`, `""`, `null`, `undefined`) | `undefined` | Attribute is omitted (`<div>`) |

### Example

```svelte
<script lang="ts">
  import { boolAttr } from "runed";
  let isActive = $state(true);
  let isLoading = $state(false);
</script>

<!-- Renders as: <div data-active> -->
<div data-active={boolAttr(isActive)}>Active content</div>

<!-- Renders as: <div> (no data-loading attribute) -->
<div data-loading={boolAttr(isLoading)}>Loading content</div>
```

### Gotchas

- **Use it for HTML boolean attributes** (`disabled`, `checked`, `readonly`, `multiple`, `required`, `autoplay`, `controls`, `hidden`, `itemscope`, and custom `data-*` attributes that you select with `[data-...]` in CSS).
- **Don't use it for ARIA attributes** like `aria-hidden` — those use `"true"` / `"false"` string values, not boolean presence. Use a ternary: `aria-hidden={isLoading ? "true" : "false"}`.
- **Returns `""` (not `true`)** for truthy values because HTML boolean attributes use the empty-string convention. Svelte handles `""` correctly.

---

## onCleanup

Register a cleanup function that runs when the current effect context is disposed (component destroyed or root effect disposed). Shorthand for `$effect(() => () => { ... })`.

### Signature

```ts
import { onCleanup } from "runed";

function onCleanup(cb: () => void): void;
```

### Example — Replacement for `onDestroy`

```svelte
<script lang="ts">
  import { onCleanup } from "runed";

  onCleanup(() => console.log("Component is being cleaned up!"));

  // Also works inside $effect.root:
  $effect.root(() => {
    onCleanup(() => console.log("Root effect is being cleaned up!"));
  });
</script>
```

### Example — Cleanup Inside an Effect

```svelte
<script lang="ts">
  import { onCleanup, useEventListener } from "runed";

  $effect(() => {
    const id = setInterval(() => console.log("tick"), 1000);
    onCleanup(() => clearInterval(id));   // runs when effect re-runs OR component destroys
  });
</script>
```

### Gotchas

- **Must be called within an effect context** (component init, `$effect` body, or `$effect.root`). Calling it outside will throw or silently fail.
- **Can replace Svelte's `onDestroy`.** Use it for cleaner cleanup symmetry.
- **Inside `$effect`, `onCleanup` runs *before* the effect re-runs** (i.e., when deps change), not just on component destroy. This is the same behavior as returning a cleanup function from `$effect`.
- **The `resource` fetcher receives its own `onCleanup` parameter** — that one is scoped to "before this fetcher re-runs", not component teardown. Don't confuse them.

---

## useDebounce

Higher-order function that returns a debounced version of a callback. Delays execution until after `delay` ms of inactivity.

Use `useDebounce` when you want to debounce a *callback*. Use `Debounced` when you want a debounced *value*.

### Signature

```ts
import { useDebounce } from "runed";

useDebounce(
  callback: () => void,
  delay: MaybeGetter<number | undefined>
): ((...args: any[]) => void) & {
  pending: boolean;
  cancel: () => void;
  runScheduledNow: () => void;
};
```

### Returned Function Properties

| Member | Description |
| --- | --- |
| `(call)` | Calling the returned function schedules the callback. |
| `.pending` | Boolean; whether a call is currently scheduled. |
| `.cancel()` | Cancels the pending call. |
| `.runScheduledNow()` | Runs the pending call immediately (instead of waiting for the timer). |

### Example

```svelte
<script lang="ts">
  import { useDebounce } from "runed";

  let count = $state(0);
  let debounceDuration = $state(1000);

  const logCount = useDebounce(
    () => { console.log(`Pressed ${count} times`); count = 0; },
    () => debounceDuration
  );
</script>

<button onclick={() => { count++; logCount(); }}>DING</button>
<button onclick={logCount.runScheduledNow} disabled={!logCount.pending}>Run now</button>
<button onclick={logCount.cancel} disabled={!logCount.pending}>Cancel</button>
```

### Gotchas

- **`delay` is a `MaybeGetter`.** Pass `() => debounceDuration` for reactive delay.
- **`runScheduledNow` only fires if there's a pending call.** Gate it with `.pending` to avoid no-ops.
- **The callback does not receive arguments.** If you need to pass args, capture them in the closure (or use a ref).
- **For a debounced *value* (not a callback), use `Debounced`.** It exposes `.current` for template binding.

---

## useInterval

Reactive wrapper around `setInterval` with pause/resume, a built-in tick counter, optional callback, and reactive delay.

### Signature

```ts
import { useInterval } from "runed";

useInterval(
  delay: MaybeGetter<number>,
  options?: {
    immediate?: boolean;             // default true
    immediateCallback?: boolean;     // default false
    callback?: (count: number) => void;
  }
);
```

### Properties / Methods

| Member | Description |
| --- | --- |
| `counter` | Number of ticks since last reset. |
| `isActive` | Boolean; true when running. |
| `pause()` | Pause the interval. |
| `resume()` | Resume the interval. |
| `reset()` | Reset the counter to 0. |

### Configuration Options

| Option | Type | Default | Description |
| --- | --- | --- | --- |
| `immediate` | `boolean` | `true` | Whether to start the interval immediately on creation. |
| `immediateCallback` | `boolean` | `false` | Whether to execute the callback immediately when resuming. |
| `callback` | `(count: number) => void` | `undefined` | Called on each tick with the current counter value. |

### Example

```svelte
<script lang="ts">
  import { useInterval } from "runed";
  let delay = $state(1000);

  const interval = useInterval(() => delay, {
    callback: (count) => console.log(`Tick ${count}`),
  });
</script>

<p>Counter: {interval.counter}</p>
<p>Status: {interval.isActive ? "Running" : "Paused"}</p>
<button onclick={interval.pause} disabled={!interval.isActive}>Pause</button>
<button onclick={interval.resume} disabled={interval.isActive}>Resume</button>
<button onclick={interval.reset}>Reset Counter</button>
```

### Example — Reactive Delay

```svelte
<script lang="ts">
  import { useInterval } from "runed";
  let speed = $state(1);   // ticks per second
  const ticks = useInterval(() => 1000 / speed);
</script>

<input type="range" min="0.5" max="10" step="0.5" bind:value={speed} />
<p>Speed: {speed}/s | Ticks: {ticks.counter}</p>
```

### Gotchas

- **Delay is reactive.** Changing it automatically restarts the timer with the new interval — no manual `clearInterval` / `setInterval` needed.
- **`immediateCallback` runs the callback once on `resume()`** (not just on the next tick). Useful when you want immediate feedback after unpausing.
- **Pass `delay` as a number or getter.** For static delay, `useInterval(1000)` works. For reactive, `useInterval(() => delay)`.
- **`counter` is for display convenience.** For accurate "how many times has this fired" tracking, use your own counter in the callback (the `counter` resets on `reset()`).

---

## useThrottle

Higher-order function that returns a throttled version of a callback. Limits execution to at most once per `delay` ms.

Use `useThrottle` when you want to throttle a *callback*. Use `Throttled` when you want a throttled *value*.

### Signature

```ts
import { useThrottle } from "runed";

useThrottle(
  callback: () => void,
  delay: MaybeGetter<number | undefined>
): (...args: any[]) => void;
```

### Example

```svelte
<script lang="ts">
  import { useThrottle } from "runed";

  let search = $state("");
  let throttledSearch = $state("");
  let durationMs = $state(1000);

  const throttledUpdate = useThrottle(
    () => { throttledSearch = search; },
    () => durationMs
  );
</script>

<input
  value={search}
  oninput={(e) => { search = e.currentTarget.value; throttledUpdate(); }}
/>
<p>You searched for: <b>{throttledSearch}</b></p>
```

### Gotchas

- **Unlike `useDebounce`, the docs do not explicitly list extra methods** (`.pending`, `.cancel`, `.runScheduledNow`) on the returned function. Treat it primarily as a callable.
- **`delay` is a `MaybeGetter`.** Pass `() => durationMs` for reactive throttle.
- **For a throttled *value* (not a callback), use `Throttled`.** It exposes `.current` for template binding.
- **Throttle vs. debounce:** throttle fires at most once per `delay` (good for "live update at 1Hz"); debounce waits for a quiet period (good for "user stopped typing"). Pick based on UX.

---

## IsMounted

A class that returns the mounted state of the component it's instantiated in — `false` during SSR/initial render, `true` after mount.

Use it to gate client-only content and avoid SSR hydration mismatches.

### Signature

```ts
import { IsMounted } from "runed";

new IsMounted();
// exposes a reactive boolean via .current
```

### Example

```svelte
<script lang="ts">
  import { IsMounted } from "runed";
  const isMounted = new IsMounted();
</script>

{#if isMounted.current}
  <ClientOnlyChart data={data} />
{:else}
  <div class="skeleton">Loading chart…</div>
{/if}
```

### Example — Wrapping a DOM-Only Utility

```svelte
<script lang="ts">
  import { IsMounted, ScrollState } from "runed";

  const isMounted = new IsMounted();
  let el = $state<HTMLElement>();

  // ScrollState is DOM-only — gate it
  const scroll = $derived(
    isMounted.current ? new ScrollState({ element: () => el }) : null
  );
</script>

<div bind:this={el} style="overflow: auto; height: 200px;">
  <!-- long content -->
</div>

{#if scroll}
  <p>Scroll Y: {scroll.y}</p>
{/if}
```

### Gotchas

- **`isMounted.current` is `false` during SSR and the initial client render**, then becomes `true` after mount. Use it to gate client-only rendering.
- **Equivalent to `onMount` + `$state`** — `IsMounted` is the canonical replacement for that hand-rolled pattern.
- **Avoids hydration mismatches** by ensuring the server-rendered HTML and the first client render produce the same output (both `false`), then updating after mount.
- **Don't use it for "do something on mount"** — for that, use `onMount` or `$effect`. `IsMounted` is for *conditional rendering* based on mount state.
