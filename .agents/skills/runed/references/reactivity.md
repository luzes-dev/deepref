# Reactivity Utilities

This file documents the four Reactivity-category utilities in Runed. Read it when the user wants to:
- Resolve a `MaybeGetter` (value-or-getter) to a plain value
- Run an effect only when specific reactive deps change (vs. `$effect`'s auto-tracking)
- Async-fetch data that re-runs when reactive deps change
- Type-safely read/write URL search params in SvelteKit

## Table of Contents

- [extract](#extract)
- [resource](#resource)
- [useSearchParams](#usesearchparams)
- [watch (and watch.pre, watchOnce)](#watch)

---

## extract

Resolve the value of a getter or static variable — eliminates repetitive `typeof wait === "function" ? wait() : wait` boilerplate.

When writing utility functions whose options may be either a static value or a reactive getter (`MaybeGetter<T>`), `extract` resolves both forms to a plain value with an optional fallback.

### Signature

```ts
function extract<T>(input: MaybeGetter<T | undefined>, fallback: T): T;
function extract<T>(input: MaybeGetter<T | undefined>): T | undefined;
```

### Behavior Table

| Case | Result |
| --- | --- |
| `input` is a value | Returns the value |
| `input` is `undefined` | Returns the fallback |
| `input` is a function returning a value | Returns the function result |
| `input` is a function returning `undefined` | Returns the fallback |

The fallback is *optional*. If you omit it, `extract()` returns `T | undefined`.

### Example

```ts
import { extract } from "runed";

function throwConfetti(intervalProp?: MaybeGetter<number | undefined>) {
  // Always returns a number — falls back to 100 if undefined or getter returns undefined
  const interval = $derived(extract(intervalProp, 100));
  // ... use interval
}
```

### When to use

- When authoring your own utility that accepts `MaybeGetter<T>` options.
- When you want a clean one-liner instead of the `typeof === "function"` ternary.

Wrap the result in `$derived` if you want downstream reactivity — `extract` itself does not establish a dependency on its own.

---

## resource

Watch for changes and run async data fetching. Built on top of `watch`, it combines reactive state management with async data fetching, with automatic request cancellation, loading/error states, debouncing/throttling, and custom cleanup.

In SvelteKit, prefer `load` functions for data fetching. Use `resource` when you need component-level reactive fetching (e.g., the fetch is driven by reactive state that changes after navigation, or by user input).

### Signature

```ts
function resource<
  Source,
  RefetchInfo = unknown,
  Fetcher extends ResourceFetcher<Source, Awaited<ReturnType<Fetcher>>, RefetchInfo>
    = ResourceFetcher<Source, any, RefetchInfo>
>(
  source: Getter<Source>,
  fetcher: Fetcher,
  options?: ResourceOptions<Awaited<ReturnType<Fetcher>>>
): ResourceReturn<Awaited<ReturnType<Fetcher>>, RefetchInfo>;
```

`source` may be a single getter or an **array of getters** for multiple dependencies; the fetcher then receives a tuple.

### Fetcher Signature

```ts
type ResourceFetcher<Source, Data, RefetchInfo> = (
  value: Source,                              // current value (or tuple for multi-source)
  previousValue: Source | undefined,          // previous value (or tuple, or undefined first run)
  info: {
    data: Data | undefined;                   // previous returned data
    refetching: RefetchInfo | boolean;        // whether this is a refetch (or the value passed to refetch())
    onCleanup: (fn: () => void) => void;      // cleanup before this fetcher re-runs
    signal: AbortSignal;                      // for cancelling fetch() requests
  }
) => Promise<Data>;
```

### Return Type

```ts
type ResourceReturn<Data, RefetchInfo> = {
  current: Data | undefined;                  // current value
  loading: boolean;                           // true while fetch is in flight
  error: Error | undefined;                   // set if fetch threw
  mutate: (value: Data) => void;              // set value directly (optimistic updates)
  refetch: (info?: RefetchInfo) => Promise<Data | undefined>;  // re-run fetcher with current sources
};
```

### Configuration Options (`ResourceOptions`)

| Option | Type | Behavior |
| --- | --- | --- |
| `lazy` | `boolean` | Skip the initial fetch; only fetch on dependency change or `refetch()`. |
| `once` | `boolean` | Only fetch once; ignore subsequent dependency changes. |
| `initialValue` | `Data` | Provides a value before the first fetch completes. |
| `debounce` | `number` (ms) | Debounce rapid changes; cancels pending requests, runs only the last after the delay. |
| `throttle` | `number` (ms) | Throttle rapid changes; spaces requests at least by delay, returns pending promise if called too soon. |

### Example — Single Source

```svelte
<script lang="ts">
  import { resource } from "runed";

  let id = $state(1);

  const post = resource(
    () => id,
    async (id, _prevId, { signal }) => {
      const response = await fetch(`api/posts?id=${id}`, { signal });
      return response.json();
    },
    { debounce: 300 }
  );
</script>

<input type="number" bind:value={id} />

{#if post.loading}
  <div>Loading…</div>
{:else if post.error}
  <div>Error: {post.error.message}</div>
{:else}
  <article>{post.current?.title}</article>
{/if}
```

### Example — Multiple Sources (Tuple)

```ts
const results = resource([() => query, () => page], async ([query, page]) => {
  const res = await fetch(`/api/search?q=${query}&page=${page}`);
  return res.json();
});
```

### Example — Custom Cleanup (EventSource stream)

```ts
const stream = resource(
  () => streamId,
  async (id, _, { signal, onCleanup }) => {
    const eventSource = new EventSource(`/api/stream/${id}`);
    onCleanup(() => eventSource.close());        // runs before fetcher re-runs
    const res = await fetch(`/api/stream/${id}/init`, { signal });
    return res.json();
  }
);
```

### Example — Pre-render Execution

```ts
const data = resource.pre(
  () => query,
  async (query) => {
    const res = await fetch(`/api/search?q=${query}`);
    return res.json();
  }
);
```

`resource.pre` uses `$effect.pre` — runs before DOM updates. Useful when you need the data in the same render cycle that the dependency changed.

### Gotchas

- **Don't pass both `debounce` and `throttle`.** If both are specified, `debounce` takes precedence and `throttle` is silently ignored. Pick one.
- **The fetcher's `onCleanup` is per-fetcher-run, not per-component.** It runs *before the fetcher re-runs* (e.g., when deps change and an in-flight request is being cancelled). For component-lifetime cleanup, use the global `onCleanup` from `"runed"`.
- **`signal` is for `fetch()`.** Pass `{ signal }` to `fetch()` so in-flight requests are aborted when deps change. If you're using a non-`fetch` API (e.g., `XMLHttpRequest`), wire up the abort manually.
- **`mutate` is for optimistic updates.** Calling `mutate(newValue)` sets `current` immediately without re-fetching. Use it when you've predicted the server's response and want instant UI feedback.
- **`refetch()` accepts an info argument** that becomes `info.refetching` in the fetcher — useful for tagging the reason ("manual-button-click", "websocket-invalidated", etc.).

---

## useSearchParams

Reactive, type-safe, schema-validated URL search parameters for SvelteKit. Built on [Standard Schema](https://standardschema.dev/) so it works with Zod ≥ 3.24, Valibot, Arktype, or Runed's own `createSearchParamsSchema`.

**Import path:** `runed/kit` (requires `@sveltejs/kit`).

### Signature

```ts
import { useSearchParams, validateSearchParams, createSearchParamsSchema } from "runed/kit";

useSearchParams(schema: StandardSchemaV1, options?: SearchParamsOptions): ReturnUseSearchParams<Schema>;

validateSearchParams(url, schema, options?): { searchParams: URLSearchParams; data: ... };

createSearchParamsSchema(config: SchemaTypeConfig): /* StandardSchema */;
```

### Returned Object

- Direct property access: `params.page = 2` updates the URL; `const page = $derived(params.page)` reads reactively.
- `params.update(values: Partial<...>)` — batch update (single URL update).
- `params.reset(showDefaults?)` — reset all to defaults.
- `params.toURLSearchParams()` — returns a `URLSearchParams` snapshot.

### Configuration Options (`SearchParamsOptions`)

| Option | Type | Default | Behavior |
| --- | --- | --- | --- |
| `showDefaults` | `boolean` | `false` | If true, params with default values are shown in URL; otherwise omitted. |
| `debounce` | `number` (ms) | `0` | Delay URL updates to avoid cluttering history (good for typing). |
| `pushHistory` | `boolean` | `true` | If true, each update creates a new history entry; if false, replaces current URL. |
| `compress` | `boolean` | `false` | Compress all params into a single `_data` param using lz-string. |
| `compressedParamName` | `string` | `"_data"` | Name of compressed param (customize to avoid schema conflicts). |
| `updateURL` | `boolean` | `true` | If false, params are in-memory only (compress ignored). |
| `noScroll` | `boolean` | `false` | Preserve scroll position on URL update (no jump-to-top). |
| `dateFormats` | `Record<string, "date" \| "datetime">` | `undefined` | Per-field date format: `"date"` → `YYYY-MM-DD`, `"datetime"` → full ISO8601 (default). |

### Example — Zod Schema

```svelte
<script lang="ts">
  import { z } from "zod";
  import { useSearchParams } from "runed/kit";

  const productSearchSchema = z.object({
    page: z.coerce.number().default(1),
    filter: z.string().default(""),
    sort: z.enum(["newest", "oldest", "price"]).default("newest"),
  });

  const params = useSearchParams(productSearchSchema);
</script>

<input type="text" bind:value={params.filter} />
<select bind:value={params.sort}>
  <option value="newest">Newest</option>
  <option value="oldest">Oldest</option>
  <option value="price">Price</option>
</select>
```

### Example — Server-side Validation

```ts
import { validateSearchParams } from "runed/kit";

export async function load({ url }) {
  const { data } = validateSearchParams(url, productSearchSchema);
  // data is fully typed and validated
  return { products: await fetchProducts(data.page, data.filter, data.sort) };
}
```

### URL Storage Format

- Arrays → JSON strings
- Objects → JSON strings
- Dates → ISO8601 (or `YYYY-MM-DD` if `dateFormats[field] === "date"`)
- Primitives → stored directly

### Gotchas

- **Top-level reactivity only.** Direct property assignment works (`params.page = 2`); nested mutations do NOT trigger URL updates:
  - `params.config.theme = "dark"` ❌
  - `params.config = { ...params.config, theme: "dark" }` ✅
  - Array methods (`push`, `pop`, etc.), item mutations (`params.tags[0] = "x"`), and `delete` also don't trigger.
- **`createSearchParamsSchema` limitations:** arrays don't validate items; objects don't validate nested props; no custom rules; nested property changes require full reassignment. Prefer Zod/Valibot/Arktype if you need full validation power.
- **`validateSearchParams` does not modify the URL.** It returns `{ searchParams, data }` for server-side reads. If you use `compress: true` client-side, pass the same `compressedParamName` server-side so the compressed blob can be decoded.
- **Zod v4.1.0+ codecs** enable custom encode/decode (Unix timestamps, base36 IDs, etc.) and work automatically with `validateSearchParams` server-side.
- **`debounce` is your friend for text inputs** — without it, every keystroke creates a new history entry, which is unusable.

---

## watch

Like `$effect`, but lets you **manually specify which values trigger the callback** (via a getter or array of getters) rather than auto-tracking all reads. Receives current and previous source values.

Use `watch` when:
- You want an effect to run only when a specific subset of your reactive state changes (not every read inside the callback).
- You need the *previous* value of a dependency in your effect.
- You want cleaner intent than `$effect` + manual tracking.

### Signature

```ts
import { watch, watchOnce } from "runed";

watch(sources, callback, options?);
```

- `sources` is `Getter<S>` or `Array<Getter<S>>`.
- `callback` is `(current, previous) => void`. `current` and `previous` are tuples if `sources` is an array.

### Options

| Option | Type | Default | Behavior |
| --- | --- | --- | --- |
| `lazy` | `boolean` | `false` | If `true`, the first run only happens after sources change (skips initial run). |

### Variants

- `watch.pre` — same as `watch` but uses Svelte's `$effect.pre` (runs before DOM updates). Useful when you need to measure or mutate the DOM before Svelte applies its reactive updates.
- `watchOnce` — runs the callback only once, then auto-stops. Does NOT accept an options object.
- `watchOnce.pre` — pre-render version of `watchOnce`.

### Example — Single Source

```ts
let count = $state(0);

watch(() => count, (curr, prev) => {
  console.log(`count is ${curr}, was ${prev}`);
});
```

### Example — Multiple Sources

```ts
watch([() => age, () => name], ([age, name], [prevAge, prevName]) => {
  console.log(`age: ${age} (was ${prevAge}), name: ${name} (was ${prevName})`);
});
```

### Example — Deep Watch of an Object

Svelte's `$state` proxies are not deeply equal across mutations, so to watch an entire object's contents, snapshot it:

```ts
let user = $state({ name: "Alice", age: 30 });

watch(() => $state.snapshot(user), (curr, prev) => {
  console.log("user changed:", curr, "was:", prev);
});
```

### Gotchas

- **The callback's second arg is the *previous* value**, not the *current* value. Read order carefully when destructuring.
- **For deep watching, use `$state.snapshot()`.** Without it, the getter returns the same proxy object on every call, so `watch` won't detect internal mutations.
- **`watchOnce` does not accept options.** It just runs once — pass the same callback signature.
- **`watch` runs after render by default.** If you need pre-render execution (e.g., to measure the DOM before paint), use `watch.pre`.
- **`watch` is the foundation for `resource`.** If you find yourself writing "watch a dep, then async-fetch", reach for `resource` instead — it handles cancellation, loading, and error states for you.
