# State Utilities

This file documents the seven State-category utilities in Runed. Read it when the user wants to:
- Share data across the component tree without prop drilling (Context)
- Get a debounced or throttled *value* (Debounced, Throttled)
- Model state machines with type-safe transitions (FiniteStateMachine)
- Persist state to localStorage/sessionStorage with cross-tab sync (PersistedState)
- Access the previous value of a reactive getter (Previous)
- Add undo/redo to a state value (StateHistory)

## Table of Contents

- [Context](#context)
- [Debounced](#debounced)
- [FiniteStateMachine](#finitestatemachine)
- [PersistedState](#persistedstate)
- [Previous](#previous)
- [StateHistory](#statehistory)
- [Throttled](#throttled)

---

## Context

Type-safe wrapper around Svelte's Context API for sharing data across the component tree without prop drilling. Useful for themes, auth, localization, settings, or any data that many descendants need but only one ancestor provides.

### Signature

```ts
import { Context } from "runed";

class Context<TContext> {
  constructor(name: string) {}
  get key(): symbol;
  exists(): boolean;
  get(): TContext;                  // throws if not set
  getOr<TFallback>(fallback: TFallback): TContext | TFallback;
  set(context: TContext): TContext;
}
```

The constructor `name` is only used for the context key + error messages — it doesn't restrict what you can store.

### Example

```ts
// theme-context.ts
import { Context } from "runed";
export const themeContext = new Context<"light" | "dark">("theme");
```

```svelte
<!-- +layout.svelte -->
<script lang="ts">
  import { themeContext } from "./theme-context";
  let { data, children } = $props();
  themeContext.set(data.theme);   // set during init — OK
</script>
{@render children()}
```

```svelte
<!-- +page.svelte -->
<script lang="ts">
  import { themeContext } from "./theme-context";
  const theme = themeContext.get();   // or themeContext.getOr("light")
</script>
<p>Current theme: {theme}</p>
```

### Gotchas

- **Must be called during component initialization.** Same rule as Svelte's `onMount`. You cannot call `set()`, `get()`, `exists()`, or `getOr()` inside event handlers, async callbacks, or after `await`. Set context in the top-level `<script>` of a parent component.
- **`get()` throws if not set.** Use `getOr(fallback)` if the context might be absent (e.g., when a component is reused outside the provider).
- **`exists()` is reactive-safe** — you can use it to conditionally render provider/placeholder UI.

---

## Debounced

A class wrapper over `useDebounce` that exposes a debounced reactive *value*. Input changes don't propagate to `.current` until after a quiet period of `delay` ms.

Use `Debounced` when you want a debounced *value* in your template (e.g., a search query). Use `useDebounce` when you want to debounce a *callback* (e.g., a side effect).

### Signature

```ts
import { Debounced } from "runed";

new Debounced<T>(getter: Getter<T>, delay: MaybeGetter<number | undefined>);
```

`delay` is a `MaybeGetter` — pass a number, a getter returning a number, or omit (no debounce).

### Properties / Methods

| Member | Description |
| --- | --- |
| `current` | The debounced value (read in template / `$derived`). |
| `cancel()` | Cancels pending update; current value stays as-is. |
| `setImmediately(value)` | Sets the value right now, cancelling any pending update. |
| `updateImmediately()` | Async; flushes the pending update immediately (without waiting for the timer). |

### Example

```svelte
<script lang="ts">
  import { Debounced } from "runed";

  let search = $state("");
  const debounced = new Debounced(() => search, 500);
</script>

<input bind:value={search} />
<p>You searched for: <b>{debounced.current}</b></p>
```

### Gotchas

- **`setImmediately` and `updateImmediately` also cancel pending updates.** Don't call them expecting both immediate flush and a queued timer — pick one.
- **`delay` is reactive.** Pass `() => debounceTime` if the user can change the debounce duration at runtime.
- **`Previous.current` is unrelated.** Don't confuse `Debounced.current` (the debounced latest) with `Previous.current` (the value before the latest change).

---

## FiniteStateMachine

A strongly-typed, intentionally minimalistic finite state machine. Declare states, the events that trigger transitions between them, lifecycle hooks (`_enter`, `_exit`), wildcard handlers, and debounced transitions.

The docs describe it as "a loving rewrite of [kenkunz/svelte-fSM]" and explicitly recommend `statelyai/xstate` for more powerful needs.

### Signature

```ts
import { FiniteStateMachine } from "runed";

new FiniteStateMachine<MyStates, MyEvents>(initialState, config);
```

Where `config` is:

```ts
{
  [state]: {
    [event]: nextState | action,        // nextState string, or action function
    _enter?: (meta) => void,             // runs on entering this state
    _exit?: (meta) => void,              // runs on exiting this state
  },
  "*"?: { [event]: nextState }           // fallback handlers for any state
}
```

An *action* is a function returning the next state (or returning nothing to **prevent** the transition). Actions receive extra params passed via `send(event, ...args)`.

`meta = { from, to, event, args? }`. For the initial state's `_enter`, `from = event = null`.

### Methods

| Method | Description |
| --- | --- |
| `send(event, ...args)` | Dispatch an event; transitions to the mapped state (or runs the action). Extra args are passed to the action. |
| `debounce(ms, event)` | Schedule an event to fire after `ms`; re-invoking with the same event cancels and restarts the timer. |

### Example — Toggle

```ts
type States = "off" | "on";
type Events = "toggle";

const f = new FiniteStateMachine<States, Events>("off", {
  off: { toggle: "on" },
  on:  { toggle: "off" },
});

f.send("toggle");  // off → on
f.send("toggle");  // on → off
```

### Example — Lifecycle Hooks and Wildcard

```ts
type States = "idle" | "loading" | "success" | "error";
type Events = "fetch" | "resolve" | "reject" | "reset";

const fsm = new FiniteStateMachine<States, Events>("idle", {
  idle:    { fetch: "loading" },
  loading: {
    resolve: "success",
    reject:  "error",
    _exit:   () => console.log("leaving loading"),
  },
  success: { reset: "idle", _enter: () => showToast("Done!") },
  error:   { reset: "idle" },
  "*":     { reset: "idle" },   // any state can reset
});
```

### Example — Actions with Args (Conditional Transitions)

```ts
const fsm = new FiniteStateMachine<States, Events>("idle", {
  idle: {
    fetch: (meta, url: string) => {
      if (!url) return;          // returning nothing prevents transition
      startFetch(url);
      return "loading";
    },
  },
});

fsm.send("fetch", "/api/data");
```

### Gotchas

- **Returning `undefined` (or nothing) from an action prevents the transition.** Use this for guarded transitions ("don't switch to loading if the URL is empty").
- **The initial state's `_enter` runs immediately on FSM creation** with `meta.from = null` and `meta.event = null`. Guard accordingly if you don't want side effects during construction.
- **`debounce(ms, event)`** is great for "save after 500ms of inactivity" patterns — re-calling with the same event restarts the timer.
- **Wildcard `"*"` provides fallbacks.** If the current state has no handler for an event, the wildcard is consulted. Use it for cross-cutting events like "reset" or "cancel".

---

## PersistedState

Reactive state container that persists to `localStorage` (default) or `sessionStorage`, with optional cross-tab synchronization via the storage event.

### Signature

```ts
import { PersistedState } from "runed";

new PersistedState<T>(key: string, initialValue: T, options?: PersistedStateOptions);
```

### Properties / Methods

| Member | Description |
| --- | --- |
| `current` | Get/set the persisted value. Mutating `state.current++` persists. |
| `connected` | Boolean reflecting current connection status. |
| `connect()` | Connect to storage; immediately persists the in-memory value. |
| `disconnect()` | Removes the value from storage but preserves it in memory; disables cross-tab sync. |

### Configuration Options

| Option | Default | Behavior |
| --- | --- | --- |
| `storage` | `"local"` | `"local"` (persists until cleared) or `"session"` (persists until session ends). |
| `syncTabs` | `true` | Auto-sync changes across tabs via the storage event. |
| `connected` | `true` | Start connected; set `false` to keep state in memory only until `connect()` is called. |
| `serializer` | JSON | `{ serialize, deserialize }` — e.g., use `superjson` for Date objects. |

### Example — Simple Counter

```svelte
<script lang="ts">
  import { PersistedState } from "runed";
  const count = new PersistedState("count", 0);
</script>

<button onclick={() => count.current++}>Increment</button>
<p>Count: {count.current}</p>
```

### Example — Custom Serializer (for Date objects)

```ts
import superjson from "superjson";

const session = new PersistedState("session", { user: null, expiresAt: null }, {
  serializer: superjson,
});
```

### Gotchas

- **Class instances are NOT deeply reactive.** Plain objects and arrays persist on mutation, but class instances do not — you must reassign the whole value: `persisted.current = new Person("JG")` ✅. Mutating `persisted.current.name = "JG"` ❌ will update in-memory state but won't trigger storage write.
- **`disconnect()` removes the value from storage but keeps it in memory.** This is *not* a "delete the key" operation — it's "stop syncing". Use it when the user logs out (clear persisted identity) but you still want the in-memory value during teardown.
- **`syncTabs: true` (default) means changes propagate to other tabs in real time.** This is great for "log out everywhere" patterns but can surprise users if not expected. Set `syncTabs: false` if cross-tab sync is undesired.
- **The `serializer` must round-trip.** If you store Dates with JSON, they come back as strings. Use `superjson` (or a custom `{ serialize, deserialize }`) for non-JSON-native types.
- **Initial value is written to storage on first construction** (if no value exists yet). After that, the stored value wins.

---

## Previous

Tracks and exposes the *previous* value of a reactive getter. Useful for diffing, transition effects ("animate when value increases"), or building undo primitives.

### Signature

```ts
import { Previous } from "runed";

class Previous<T> {
  constructor(getter: () => T);
  readonly current: T | undefined;   // previous value
}
```

### Example

```svelte
<script lang="ts">
  import { Previous } from "runed";
  let count = $state(0);
  const previous = new Previous(() => count);
</script>

<button onclick={() => count++}>Count: {count}</button>
<pre>Previous: {`${previous.current}`}</pre>
```

### Gotchas

- **`current` is `T | undefined`** on the very first read — there is no previous value yet. Guard for `undefined` in templates and downstream logic:
  ```svelte
  {#if previous.current !== undefined && count > previous.current}
    ↑ increased
  {/if}
  ```
- **`Previous` only stores one step back.** For multi-step history (with undo/redo), use `StateHistory`.
- **The getter is called inside a reactive context**, so reading a `$state` variable inside it sets up a dependency. `Previous` updates whenever the getter's return value changes.

---

## StateHistory

Tracks a getter's return value over time, logging each change into an array and providing undo/redo.

### Signature

```ts
import { StateHistory } from "runed";

new StateHistory<T>(getter: Getter<T>, setter: (v: T) => void);
```

The `setter` is required for undo/redo to apply changes back to the source state.

### Properties / Methods

| Member | Description |
| --- | --- |
| `log` | Array of `LogEvent<T> = { snapshot: T, timestamp: ... }`. |
| `canUndo` | Derived boolean — true when `log.length > 1` (need somewhere to go back to). |
| `canRedo` | Derived boolean — true when redo stack is non-empty. |
| `undo()` | Revert to previous value; current state moves to redo stack. |
| `redo()` | Restore a previously undone state. |
| `clear()` | Clear log + redo stack. |

### Example

```svelte
<script lang="ts">
  import { StateHistory } from "runed";
  let count = $state(0);
  const history = new StateHistory(() => count, (c) => (count = c));
</script>

<button onclick={() => count++}>Increment</button>
<button disabled={!history.canUndo} onclick={history.undo}>Undo</button>
<button disabled={!history.canRedo} onclick={history.redo}>Redo</button>
<button onclick={history.clear}>Clear History</button>
<p>Count: {count}</p>
```

### Gotchas

- **`canUndo` requires `log.length > 1`.** With only the initial entry, there's nowhere to go back to.
- **The setter must mutate the source state directly.** Pass `(c) => (count = c)` — not `(c) => someOtherVar = c`.
- **`undo`/`redo` push to opposite stacks.** Calling `undo` moves the current state to the redo stack; calling `redo` moves it back to the undo stack.
- **`clear` resets both stacks.** Use it when the user explicitly wants to "forget history" (e.g., after saving).

---

## Throttled

A class wrapper over `useThrottle` that exposes a throttled reactive *value*. Input propagates to `.current` at most once per `delay` ms.

Use `Throttled` for a throttled *value* (e.g., a live-updating cursor position). Use `useThrottle` for a throttled *callback*.

### Signature

```ts
import { Throttled } from "runed";

new Throttled<T>(getter: Getter<T>, delay: MaybeGetter<number | undefined>);
```

### Properties / Methods

| Member | Description |
| --- | --- |
| `current` | The throttled value (read in template / `$derived`). |
| `cancel()` | Cancels pending update. |
| `setImmediately(value)` | Sets value now, cancelling pending updates. |

### Example

```svelte
<script lang="ts">
  import { Throttled } from "runed";
  let search = $state("");
  const throttled = new Throttled(() => search, 500);
</script>

<input bind:value={search} />
<p>You searched for: <b>{throttled.current}</b></p>
```

### Gotchas

- **`Throttled` does not document `updateImmediately()`** (unlike `Debounced`). Only `cancel()` and `setImmediately()` are available. If you need to flush immediately, use `setImmediately(throttled.current)`.
- **Throttle vs. debounce:** throttle guarantees at most one update per `delay` ms (good for live updates); debounce waits for a quiet period (good for "user stopped typing"). Pick based on the UX.
- **`delay` is reactive.** Pass `() => throttleMs` if the user can change the throttle duration at runtime.
