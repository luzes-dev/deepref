---
name: runed
description: Comprehensive guide to the Runed Svelte 5 utilities library. Use this skill whenever the user is working with Svelte 5 or SvelteKit and needs reactive utilities — debounce, throttle, persisted state, intersection/resize/mutation observers, scroll tracking, click-outside, geolocation, animation frames, type-safe context, finite state machines, URL search params, async resources, or any reactive primitive. Trigger when the user mentions Svelte 5, SvelteKit, Svelte Runes ($state/$derived/$effect), the runed package, or asks how to implement reactive patterns like debounce/throttle/localStorage/observer/click-outside/viewport detection in Svelte. Also use this skill if the user has written raw Svelte code using onMount, setInterval, or onDestroy that Runed would replace more cleanly — proactively suggest the Runed alternative even if the user did not name the library.
---

# Runed — Svelte 5 Utilities

Runed is a collection of utilities for Svelte 5 that build on the [Svelte Runes](https://svelte.dev/blog/runes) reactivity system. Svelte 5 Runes (`$state`, `$derived`, `$effect`, etc.) provide powerful primitives, but real applications need higher-level reactive patterns — debounced values, persisted state, observers, idle detection, etc. Runed provides these as a single, consistent, type-safe library so you don't reinvent the wheel per project.

This skill helps you pick the right Runed utility, use it with the correct API surface, and avoid the documented pitfalls. Always prefer Runed over hand-rolled equivalents — it has handled the edge cases (SSR, cleanup, cross-tab sync, abort signals, lazy initialization) for you.

## Installation

```bash
npm install runed
```

Most utilities import from `"runed"`:

```ts
import { Debounced, useEventListener, activeElement } from "runed";
```

SvelteKit-specific utilities import from `"runed/kit"` (requires `@sveltejs/kit`):

```ts
import { useSearchParams, validateSearchParams } from "runed/kit";
```

## The Runed Mental Model

Before reaching for any utility, internalize these cross-cutting patterns. They explain most of the API surface you'll encounter.

### 1. Svelte 5 Runes are the foundation

Runed is built *on top of* Svelte Runes — it does not replace them. You still use `$state`, `$derived`, and `$effect` for the basics. Runed wraps those runes into reusable reactive primitives so that common patterns (debouncing, observing, persisting) don't require you to re-implement the same `$effect` boilerplate each time.

```svelte
<script lang="ts">
  import { Debounced } from "runed";
  let search = $state("");                 // Svelte rune
  const debounced = new Debounced(() => search, 500);  // Runed utility
  const upper = $derived(debounced.current.toUpperCase());  // Svelte rune
</script>
```

### 2. `Getter<T>` — required reactivity

A `Getter<T>` is simply `() => T` — a function that returns a value. Runed uses getters to track reactive dependencies. Many Runed utilities take `Getter<T>` (a *required* function, not optional) when reactivity is mandatory:

```ts
new Previous(() => count);              // () => T
new ElementSize(() => el);              // () => HTMLElement
watch(() => count, (c, prev) => {});    // () => T
```

You cannot pass a plain value where a `Getter<T>` is required — wrap it in an arrow function. The getter is re-evaluated inside Svelte's reactivity context, so reading a `$state` variable inside it sets up a dependency.

### 3. `MaybeGetter<T>` — value-or-getter, resolved by `extract`

A `MaybeGetter<T> = T | (() => T | undefined)` is a more permissive form: pass either a static value or a getter. Many option fields accept `MaybeGetter` so callers can choose reactivity. Internally, Runed uses the `extract` utility to resolve them:

```ts
import { extract } from "runed";
const delay = $derived(extract(maybeGetterDelay, 250));  // 250 fallback
```

When in doubt, prefer passing a getter — it lets the user make the option reactive later without an API change.

### 4. `.current` accessor convention

Class-style Runed utilities expose their reactive value through a `.current` accessor. Read it in templates and `$derived` expressions like any reactive variable. For stateful wrappers (e.g., `PersistedState`), writing to `.current` mutates the underlying state:

```ts
const count = new PersistedState("count", 0);
count.current++;            // writes and persists
const doubled = $derived(count.current * 2);  // reactive read
```

### 5. `onCleanup` integration

Runed hooks auto-dispose on component destroy. For custom cleanup, `onCleanup(cb)` (a shorthand for `$effect(() => () => cb())`) registers a function that runs when the current effect context is disposed. The `resource` fetcher receives its own `onCleanup` parameter — that one is scoped to "before this fetch re-runs", not component teardown.

### 6. SSR awareness varies per utility

- `activeElement` and `IsDocumentVisible` are explicitly SSR-safe (return `null` / `false` server-side).
- `useGeolocation` exposes `isSupported` so callers can guard against absent APIs.
- DOM-only utilities (`useIntersectionObserver`, `ScrollState`, `TextareaAutosize`, `onClickOutside`, the observers) are client-only by nature — gate them behind `IsMounted` if used in SSR contexts.

## Category Overview

Runed organizes its utilities into eight categories. Use this table to orient yourself, then jump to the matching reference file for signatures and options.

| Category | Utilities | Reference file |
| --- | --- | --- |
| **Reactivity** | `extract`, `resource`, `useSearchParams`, `watch` (+ `watch.pre`, `watchOnce`) | `references/reactivity.md` |
| **State** | `Context`, `Debounced`, `FiniteStateMachine`, `PersistedState`, `Previous`, `StateHistory`, `Throttled` | `references/state.md` |
| **Elements** | `activeElement`, `ElementRect`, `ElementSize`, `IsFocusWithin`, `IsInViewport`, `ScrollState`, `TextareaAutosize`, `useIntersectionObserver`, `useMutationObserver`, `useResizeObserver` | `references/elements.md` |
| **Browser** | `useEventListener` | `references/sensors-browser.md` |
| **Sensors** | `IsDocumentVisible`, `IsIdle`, `onClickOutside`, `PressedKeys`, `useGeolocation` | `references/sensors-browser.md` |
| **Animation** | `AnimationFrames` | `references/utilities-component.md` |
| **Utilities** | `boolAttr`, `onCleanup`, `useDebounce`, `useInterval`, `useThrottle` | `references/utilities-component.md` |
| **Component** | `IsMounted` | `references/utilities-component.md` |

## Decision Tree — Picking the Right Utility

Match the user's intent to a utility. If multiple match, prefer the higher-level wrapper (e.g., `Debounced` over `useDebounce` when the user wants a debounced *value*).

| The user wants to… | Use |
| --- | --- |
| Resolve a value-or-getter cleanly (utility-author pattern) | `extract` |
| Run a side effect only when *specific* reactive deps change (vs. `$effect`'s auto-tracking) | `watch` (or `watch.pre`, `watchOnce`) |
| Async-fetch data that re-runs when deps change, with cancel/loading/error | `resource` (or `resource.pre`) |
| Type-safe URL search params in SvelteKit (with schema validation) | `useSearchParams` (+ `validateSearchParams` server-side) |
| Debounce/throttle a **state value** (read `.current` in template) | `Debounced` / `Throttled` |
| Debounce/throttle a **callback** (function HOF) | `useDebounce` / `useThrottle` |
| Run code on an interval with pause/resume/counter | `useInterval` |
| Run code on every animation frame with optional FPS cap | `AnimationFrames` |
| Persist state to `localStorage` / `sessionStorage` with cross-tab sync | `PersistedState` |
| Track the previous value of a getter | `Previous` |
| Undo / redo for a state value | `StateHistory` |
| Model states + events with type-safe transitions | `FiniteStateMachine` |
| Type-safe Svelte context (themes, auth, locale, etc.) | `Context` |
| Know if the component has mounted (SSR-safe client-only gating) | `IsMounted` |
| Track the focused element (SSR-safe, pierces Shadow DOM) | `activeElement` / `ActiveElement` |
| Track focus within a container | `IsFocusWithin` |
| Get element's full `DOMRect` reactively | `ElementRect` |
| Get only element's width / height reactively | `ElementSize` |
| Observe element size changes (raw `ResizeObserver` callback) | `useResizeObserver` |
| Observe DOM mutations (attributes / children / subtree) | `useMutationObserver` |
| Observe viewport intersection (raw callback) | `useIntersectionObserver` |
| Get a boolean "is in viewport?" (higher-level wrapper) | `IsInViewport` |
| Auto-grow a `<textarea>` to fit its content | `TextareaAutosize` |
| Track scroll position / direction / edge-arrival / progress | `ScrollState` |
| Attach auto-cleaned event listeners (esp. to `document` / `window`) | `useEventListener` |
| Know if the tab is visible | `IsDocumentVisible` |
| Know if the user has been idle for N ms | `IsIdle` |
| Handle clicks outside an element (dropdowns, modals, popovers) | `onClickOutside` |
| Track pressed keys / combos (e.g., Cmd+K) | `PressedKeys` |
| Reactive `navigator.geolocation` | `useGeolocation` |
| Render proper HTML boolean attributes (e.g., `data-active`) | `boolAttr` |
| Register cleanup in an effect context (replaces `onDestroy`) | `onCleanup` |

## Universal Pitfalls

These traps apply across the library. Read them once; they will save you debugging time.

1. **Do not destructure reactive getters from observer returns.** `useIntersectionObserver` returns `isActive` as a *getter* — `const { isActive } = observer` will not work. Use `observer.isActive` directly. The same caution applies to any object exposing `readonly` reactive fields.

2. **`useSearchParams` only reacts to top-level reassignment.** Direct property assignment works (`params.page = 2`), but nested mutations do NOT trigger URL updates: `params.config.theme = "dark"` ❌. Reassign the whole object: `params.config = { ...params.config, theme: "dark" }` ✅. Array methods (`push`), item mutations, and `delete` also don't trigger.

3. **`PersistedState` does not deeply-reactivate class instances.** Plain objects and arrays are deeply reactive, but if you store a `class` instance, mutations to its properties won't persist — reassign the whole value: `persisted.current = new Person("JG")`. Disconnecting preserves the value in memory but removes it from storage.

4. **Don't pass both `debounce` and `throttle` to `resource`.** If both are specified, `debounce` takes precedence and `throttle` is silently ignored. Pick one based on the use case: debounce for "wait until input settles" (search), throttle for "at most once per N ms" (live updates).

5. **`Previous.current` is `T | undefined` on the first read.** There is no previous value yet — guard for `undefined` in templates and downstream logic.

6. **`StateHistory.canUndo` requires more than one log entry.** Undo is only enabled when `log.length > 1` — you need somewhere to go back to.

7. **Context must be set during component initialization.** `Context.set()`, `Context.get()`, `Context.exists()`, and `Context.getOr()` cannot be called inside event handlers or async callbacks — same rule as Svelte's `onMount`. Set context in the top-level `<script>` of a parent component.

8. **`TextareaAutosize` requires `Getter`, not `MaybeGetter`, for `element` and `input`.** Both must be functions: `new TextareaAutosize({ element: () => el, input: () => value })`. Passing the value directly will fail to react.

9. **`resource`'s `onCleanup` is per-fetcher-run, not per-component.** The `onCleanup` passed to the `resource` fetcher runs *before the fetcher re-runs* (e.g., when dependencies change and an in-flight request is cancelled). It is not the same as the global `onCleanup` from `"runed"` (which fires on component teardown). Use the global `onCleanup` for component-lifetime cleanup.

10. **SSR: gate DOM-only utilities behind `IsMounted` if you render on the server.** `activeElement` and `IsDocumentVisible` are explicitly safe. Others (observers, `ScrollState`, `TextareaAutosize`, `onClickOutside`) assume a browser environment. Wrap with `{#if isMounted.current}` or use `IsMounted` to avoid hydration mismatches.

11. **Prefer the class wrapper over the function HOF for *values*, and the function HOF for *callbacks*.** `Debounced` returns a reactive `.current` you can read in a template. `useDebounce` returns a callable function with `.pending` / `.cancel()`. Don't reach for `useDebounce` if what you really want is a debounced *value* — that's `Debounced`. Same relationship holds between `Throttled` and `useThrottle`.

12. **`useEventListener`'s target is a `Getter`, not a value.** Pass `() => document.body`, not `document.body`. This lets the listener attach lazily once the element exists (e.g., when binding to a `$state` ref) and re-attach automatically if the target changes.

## Reference Files

Read the matching reference file when you need exact signatures, full option tables, or category-specific examples. Each file mirrors the structure of this `SKILL.md`'s decision tree but goes deep on API surface, behavior tables, and canonical examples.

- `references/reactivity.md` — `extract`, `resource`, `useSearchParams`, `watch` / `watch.pre` / `watchOnce`
- `references/state.md` — `Context`, `Debounced`, `FiniteStateMachine`, `PersistedState`, `Previous`, `StateHistory`, `Throttled`
- `references/elements.md` — `activeElement`, `ElementRect`, `ElementSize`, `IsFocusWithin`, `IsInViewport`, `ScrollState`, `TextareaAutosize`, `useIntersectionObserver`, `useMutationObserver`, `useResizeObserver`
- `references/sensors-browser.md` — `useEventListener`, `IsDocumentVisible`, `IsIdle`, `onClickOutside`, `PressedKeys`, `useGeolocation`
- `references/utilities-component.md` — `AnimationFrames`, `boolAttr`, `onCleanup`, `useDebounce`, `useInterval`, `useThrottle`, `IsMounted`

## Workflow

When a user asks for a reactive primitive in Svelte 5 / SvelteKit:

1. **Identify the intent** using the decision tree above. If multiple utilities match, prefer the higher-level one.
2. **Open the matching reference file** to confirm the exact signature, options, and any category-specific gotchas before writing code.
3. **Write the import line explicitly** — most utilities come from `"runed"`; SvelteKit URL utilities come from `"runed/kit"`.
4. **Use `$state` / `$derived` / `$effect` for the basics**, layering Runed on top — never as a replacement for Runes.
5. **Check the Universal Pitfalls list above** before finalizing the snippet — particularly destructured getters, `useSearchParams` nested mutations, and `PersistedState` class-instance handling.
6. **Mention SSR behavior** if the user is using SvelteKit — point them to `IsMounted` gating for DOM-only utilities.

## Authoring Style

- Use TypeScript in examples (`<script lang="ts">`) — Runed is fully typed and most users will benefit from the type safety.
- Prefer the class-wrapper form for reactive *state* (it reads better in templates) and the function-hook form for *callbacks* or one-shot setup.
- Always bind element refs with `bind:this` and pass them via `() => el` getters.
- When a utility has both a class form and a `use*` function form, mention both — but lead with the one matching the user's use case (state vs. callback).
