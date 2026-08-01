# Browser & Sensors Utilities

This file documents the six Browser- and Sensors-category utilities in Runed. Read it when the user wants to:
- Attach auto-cleaned event listeners (especially to `document` / `window`)
- Know if the tab is visible
- Know if the user has been idle for N ms
- Handle clicks outside an element (dropdowns, modals, popovers)
- Track pressed keys / keyboard combinations
- Reactive `navigator.geolocation`

## Table of Contents

- [useEventListener (Browser)](#useeventlistener)
- [IsDocumentVisible (Sensors)](#isdocumentvisible)
- [IsIdle (Sensors)](#isidle)
- [onClickOutside (Sensors)](#onclickoutside)
- [PressedKeys (Sensors)](#pressedkeys)
- [useGeolocation (Sensors)](#usegeolocation)

---

## useEventListener

Attaches an automatically-disposed event listener — particularly useful for elements you don't directly control (e.g., `document.body`, `window`, or refs passed from parents) where `<svelte:body />` isn't viable.

### Signature

```ts
import { useEventListener } from "runed";

useEventListener(
  target: Getter<EventTarget | null>,
  event: string,
  callback: (e: ...) => void,
  options?: AddEventListenerOptions
);
```

`target` is a getter so it can be lazy/dynamic — return `null` initially and the listener attaches once the element becomes available.

### Example — In a Non-Component Class

```ts
import { useEventListener } from "runed";

export class ClickLogger {
  #clicks = $state(0);
  constructor() {
    useEventListener(() => document.body, "click", () => this.#clicks++);
  }
  get clicks() { return this.#clicks; }
}
```

### Example — Reactive Target

```svelte
<script lang="ts">
  import { useEventListener } from "runed";
  let el = $state<HTMLElement | null>(null);

  useEventListener(
    () => el,
    "click",
    (e: MouseEvent) => console.log("clicked at", e.clientX, e.clientY),
    { capture: true }
  );
</script>

<button bind:this={el}>Click me</button>
```

### Gotchas

- **`target` is a `Getter`, not a value.** Pass `() => document.body`, not `document.body`. This lets the listener attach lazily once the element exists.
- **Listeners auto-detach when the target changes.** If the getter returns a new element, the previous listener is removed and a new one is attached.
- **Listeners auto-detach on component destruction.** No manual cleanup needed.
- **Use `<svelte:body />` for component-owned body/window listeners.** `useEventListener` shines when the target is dynamic, late-bound, or outside the component tree (e.g., inside a class instantiated in `+page.svelte.ts`).

---

## IsDocumentVisible

Reactive boolean reflecting current document visibility via the Page Visibility API (`document.hidden` + `visibilitychange` event). SSR-safe.

### Signature

```ts
import { IsDocumentVisible } from "runed";

type IsDocumentVisibleOptions = {
  window?: Window;
  document?: Document;
};

class IsDocumentVisible {
  constructor(options?: IsDocumentVisibleOptions);
  readonly current: boolean;   // true when document is visible, false when hidden
}
```

### Example

```svelte
<script lang="ts">
  import { IsDocumentVisible } from "runed";
  const visible = new IsDocumentVisible();
</script>

<p>Document visible: {visible.current ? "Yes" : "No"}</p>
```

### Example — Pausing Expensive Work When Tab Hidden

```svelte
<script lang="ts">
  import { IsDocumentVisible, useInterval } from "runed";
  const visible = new IsDocumentVisible();
  let counter = $state(0);
  const polling = useInterval(1000, {
    callback: () => { counter++; },
  });
  $effect(() => {
    if (visible.current) polling.resume();
    else polling.pause();
  });
</script>
```

### Gotchas

- **SSR-safe.** In non-browser contexts, `current` defaults to `false`. Use freely in SvelteKit without `IsMounted` gating.
- **Custom `window` / `document`** can be passed (for testing or iframes).
- **Use it to pause expensive work** when the tab is hidden — polling, video decoding, heavy animations.

---

## IsIdle

Tracks user activity (mouse/keyboard/touch/wheel) and reports whether the user is idle after a configurable timeout, plus the timestamp of the last activity.

### Signature

```ts
import { IsIdle } from "runed";

interface IsIdleOptions {
  events?: MaybeGetter<(keyof WindowEventMap)[]>;        // default: ['mousemove','mousedown','resize','keydown','touchstart','wheel']
  timeout?: MaybeGetter<number>;                          // default: 60000
  detectVisibilityChanges?: MaybeGetter<boolean>;         // default: false
  initialState?: boolean;                                 // default: false
}

class IsIdle {
  constructor(options?: IsIdleOptions);
  readonly current: boolean;          // true when user is idle
  readonly lastActive: number;        // ms since epoch
}
```

### Example

```svelte
<script lang="ts">
  import { IsIdle } from "runed";
  const idle = new IsIdle({ timeout: 1000 });
</script>

<p>Idle: {idle.current}</p>
<p>Last active: {new Date(idle.lastActive).toLocaleTimeString()}</p>
```

### Example — Auto-Away Status

```svelte
<script lang="ts">
  import { IsIdle } from "runed";
  const idle = new IsIdle({
    timeout: 5 * 60 * 1000,        // 5 minutes
    detectVisibilityChanges: true, // also treat tab-hide as idle
  });
  $effect(() => {
    if (idle.current) updateStatus("away");
    else updateStatus("online");
  });
</script>
```

### Gotchas

- **All options are `MaybeGetter`.** Use getters for reactive configuration (`() => userPrefTimeout`).
- **`lastActive` is a timestamp (ms since epoch), not a Date.** Wrap with `new Date(idle.lastActive)` for display.
- **`detectVisibilityChanges`** also treats tab-hide as idle — useful for "auto-away" patterns.
- **Default `timeout` is 60 seconds.** Override it; the default is too long for most UIs.
- **Default `events` cover mouse, keyboard, touch, and wheel.** Add `'pointermove'` or `'scroll'` if you need finer activity detection.

---

## onClickOutside

Detect clicks that occur outside a specified element's boundaries and execute a callback. Commonly for dismissible dropdowns, modals, popovers, dialogs.

### Signature

```ts
import { onClickOutside } from "runed";

export declare function onClickOutside<T extends Element = HTMLElement>(
  container: MaybeElementGetter<T>,
  callback: (event: PointerEvent | FocusEvent) => void,
  opts?: OnClickOutsideOptions
): {
  stop: () => boolean;
  start: () => boolean;
  readonly enabled: boolean;
};
```

`MaybeElementGetter<T> = MaybeGetter<T | null | undefined>`.

### Configuration Options

| Option | Type | Default | Behavior |
| --- | --- | --- | --- |
| `immediate` | `boolean` | `true` | Whether the handler is enabled by default; if `false`, call `start()` to activate. |
| `detectIframe` | `boolean` | `false` | Whether focus events from iframes trigger the callback (iframe clicks don't bubble). |
| `document` | `Document` | global `document` | Custom document object. |
| `window` | `Window` | global `window` | Custom window object. |

### Example — Dismissible Dialog

```svelte
<script lang="ts">
  import { onClickOutside } from "runed";
  let dialog = $state<HTMLDialogElement>()!;

  const clickOutside = onClickOutside(
    () => dialog,
    () => { dialog.close(); clickOutside.stop(); },
    { immediate: false }
  );

  function openDialog() {
    dialog.showModal();
    clickOutside.start();
  }
</script>

<button onclick={openDialog}>Open Dialog</button>
<dialog bind:this={dialog}>
  <button onclick={() => dialog.close()}>Close</button>
</dialog>
```

### Example — Simple Dropdown

```svelte
<script lang="ts">
  import { onClickOutside } from "runed";
  let menu = $state<HTMLElement>();
  let open = $state(false);

  onClickOutside(
    () => menu,
    () => { open = false; }
  );
</script>

<button onclick={() => open = true}>Open menu</button>
{#if open}
  <div bind:this={menu} class="menu">
    <a href="/profile">Profile</a>
    <a href="/settings">Settings</a>
  </div>
{/if}
```

### Gotchas

- **Returns `start` / `stop` / `enabled`.** `enabled` is a reactive read-only boolean.
- **`enabled` is reactive — access it directly.** Don't destructure (`const { enabled } = onClickOutside(...)`).
- **`detectIframe` is needed when the user might click into an iframe inside your app.** Without it, clicking an iframe won't trigger the callback (iframe clicks don't bubble to the parent document).
- **The callback receives a `PointerEvent | FocusEvent`.** When `detectIframe` is true, the event is a `FocusEvent` (synthesized); otherwise a `PointerEvent`.
- **`immediate: false` is the right choice for modals/dialogs** that you open programmatically — call `start()` after `showModal()`.

---

## PressedKeys

Tracks which keyboard keys (and key combinations) are currently pressed.

### Signature

```ts
import { PressedKeys } from "runed";

new PressedKeys();   // no constructor args
```

### Methods / Properties

| Member | Description |
| --- | --- |
| `has(...keys: string[]): boolean` | True if **all** given keys are pressed simultaneously. |
| `all` | Array of all currently pressed keys. |
| `onKeys(keys: string[], callback: () => void)` | Register a callback fired when a specific combination is pressed. |

### Example — Detecting Combos

```ts
const keys = new PressedKeys();

const isArrowDownPressed = $derived(keys.has("ArrowDown"));
const isCtrlAPressed = $derived(keys.has("Control", "a"));

keys.onKeys(["meta", "k"], () => {
  console.log("open command palette");
});
```

### Example — In a Component

```svelte
<script lang="ts">
  import { PressedKeys } from "runed";
  const keys = new PressedKeys();

  keys.onKeys(["Control", "s"], (e) => {
    e?.preventDefault?.();
    save();
  });
</script>

<p>Press Ctrl+S to save. Cmd+K to open palette.</p>
```

### Gotchas

- **`has` accepts multiple keys for *combination* detection** — all must be pressed. `keys.has("Control", "a")` is true only when both Control and A are down.
- **Key names follow `KeyboardEvent.key` conventions.** Use `"Control"` (not `"Ctrl"`), `"meta"` (lowercase is fine), `"ArrowDown"`, `"a"` (lowercase for letters), `" "` (space), `"Enter"`, etc.
- **Mixed casing works.** `["meta", "k"]` matches `Meta+K` regardless of shift state.
- **`onKeys` callbacks fire once per press**, not continuously while held.

---

## useGeolocation

Reactive wrapper around the browser's Geolocation API. Tracks position, errors, supports pause/resume, and exposes `isSupported`.

### Signature

```ts
import { useGeolocation } from "runed";

useGeolocation(options?: UseGeolocationOptions): UseGeolocationReturn;

type UseGeolocationReturn = {
  readonly isSupported: boolean;
  readonly position: Omit<GeolocationPosition, "toJSON">;
  readonly error: GeolocationPositionError | null;
  readonly isPaused: boolean;
  pause: () => void;
  resume: () => void;
};

type UseGeolocationOptions = Partial<PositionOptions> & {
  immediate?: boolean;        // default true
};
```

### Configuration Options (`Partial<PositionOptions> & { immediate? }`)

| Option | Type | Default | Description |
| --- | --- | --- | --- |
| `immediate` | `boolean` | `true` | Whether to start the watcher immediately; if `false`, only starts when `resume()` is called. |
| `enableHighAccuracy` | `boolean` | `false` | Use GPS if available (more battery, more accurate). |
| `maximumAge` | `number` | `0` | Maximum cached position age (ms). |
| `timeout` | `number` | `Infinity` | Time to wait for a position (ms). |

### Example

```svelte
<script lang="ts">
  import { useGeolocation } from "runed";
  const location = useGeolocation({ enableHighAccuracy: true });
</script>

{#if !location.isSupported}
  <p>Geolocation not supported on this device.</p>
{:else if location.error}
  <p>Error: {location.error.message}</p>
{:else if location.position}
  <pre>Coords: {JSON.stringify(location.position.coords, null, 2)}</pre>
  <p>Timestamp: {new Date(location.position.timestamp).toLocaleString()}</p>
{/if}

<button onclick={location.pause} disabled={!location.isSupported || location.isPaused}>Pause</button>
<button onclick={location.resume} disabled={!location.isSupported || !location.isPaused}>Resume</button>
```

### Gotchas

- **Always check `isSupported` before relying on `position`.** On unsupported devices, `position` will not populate.
- **`position.coords` includes** `accuracy`, `latitude`, `longitude`, `altitude`, `altitudeAccuracy`, `heading`, `speed` — most are `null` unless the device supports them.
- **`enableHighAccuracy: true` uses GPS** — more accurate but more battery. Use it only when needed (e.g., turn-by-turn navigation).
- **`immediate: false`** starts the watcher only when `resume()` is called. Use it when the user must explicitly opt in to location tracking (privacy UX).
- **Errors are stored in `error`** (`GeolocationPositionError` with `code` and `message`). Common codes: `1` (permission denied), `2` (position unavailable), `3` (timeout).
