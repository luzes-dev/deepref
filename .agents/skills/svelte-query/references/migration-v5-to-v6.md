# Migrate from svelte-query v5 to v6

v6 migrates the Svelte adapter from Svelte 3/4 stores to Svelte 5 runes. This is mostly mechanical but breaks every call site. Read this when the user is upgrading an existing v5 app or sees `$query.data` patterns in legacy code.

## Table of contents
- Prerequisites
- Install
- The four mechanical changes
  - 1. Wrap options in a function (the accessor pattern)
  - 2. Drop the `$` prefix when reading the result
  - 3. Replace stores with `$state` for reactive inputs
  - 4. Drop `$derived` wrappers around `createQuery`
- Disabling legacy mode
- Common migration pitfalls

## Prerequisites

- **Svelte ≥ 5.25.0**. Check `package.json`:
  ```bash
  npm ls svelte
  ```
  If below 5.25.0, upgrade Svelte and SvelteKit first.
- SvelteKit ≥ 2.x is recommended.

## Install

```bash
npm install @tanstack/svelte-query@latest
# or: pnpm add @tanstack/svelte-query@latest / yarn add / bun add
```

v6 depends on `@tanstack/query-core` v5 (installed automatically).

## The four mechanical changes

### 1. Wrap options in a function (the accessor pattern)

v5 accepted a plain options object. v6 requires a thunk — a function that returns the options object. This is how the runes adapter tracks which `$state`/`$props` flow into the options.

```diff
- const query = createQuery({
+ const query = createQuery(() => ({
    queryKey: ['todos'],
    queryFn: () => fetchTodos(),
- })
+ }))
```

TypeScript will flag every call site that still passes a plain object, so a project-wide `tsc --noEmit` will catch them all.

This applies to **every** `create*` function: `createQuery`, `createInfiniteQuery`, `createMutation`, `createQueries`. (The `*Options` helpers — `queryOptions`, `infiniteQueryOptions`, `mutationOptions` — still take a plain object, because they're just typed factories.)

### 2. Drop the `$` prefix when reading the result

v5 returned a store. You read its value with `$query.data`. v6 returns a reactive runes object — read fields directly.

```diff
  <script>
    const todos = createQuery(() => ({ /* ... */ }))
  </script>

- {#if $todos.isSuccess}
+ {#if todos.isSuccess}
    <ul>
-     {#each $todos.data.items as item}
+     {#each todos.data.items as item}
        <li>{item}</li>
      {/each}
    </ul>
  {/if}
```

Find-and-replace `$query.` → `query.` for every query/mutation you've created.

### 3. Replace stores with `$state` for reactive inputs

In v5 you used `writable`/`readable` stores for reactive inputs to queries, then derived them into the options object.

```diff
- import { writable, derived } from 'svelte/store'
+ // (no import needed — $state is a rune)

- const intervalMs = writable(1000)
+ let intervalMs = $state(1000)

- const query = createQuery(
-   derived(intervalMs, ($intervalMs) => ({
+ const query = createQuery(() => ({
    queryKey: ['refetch'],
    queryFn: async () => await fetch('/api/data').then((r) => r.json()),
-   refetchInterval: $intervalMs,
-   }))
- )
+   refetchInterval: intervalMs,
+ }))
```

The runes adapter reads `intervalMs` reactively — when it changes, the thunk re-runs and `createQuery` sees the new `refetchInterval`. No `derived` needed.

### 4. Drop `$derived` wrappers around `createQuery`

You might have wrapped the whole `createQuery` call in `$derived` to make inputs reactive. Don't — the accessor pattern handles it natively.

```diff
- const query = $derived(
-   createQuery(() => ({
-     queryKey: ['todos', filter],
-     queryFn: () => fetchTodos(filter),
-   }))
- )
+ const query = createQuery(() => ({
+   queryKey: ['todos', filter],
+   queryFn: () => fetchTodos(filter),
+ }))
```

(You may still want `$derived` for *derived values* from a query, like `const total = $derived(query.data?.length ?? 0)`.)

## Disabling legacy mode

If any of your `.svelte` files still use stores syntax (`export let`, `$store` subscriptions, `writable()` calls), Svelte 5 will treat the whole component as "legacy mode" and runes won't activate there. The v6 adapter needs runes mode.

### Per-file

Add `<svelte:options runes={true} />` at the top of each migrated component. This is the safer approach for big apps — migrate file-by-file without breaking the rest.

```svelte
<svelte:options runes={true} />

<script lang="ts">
  let count = $state(0)
</script>
```

### Project-wide

Once 100% of your components use runes, set `runes: true` in `svelte.config.js`:

```js
// svelte.config.js
export default {
  compilerOptions: {
    runes: true,
  },
  // … rest of config
}
```

This forces runes mode globally and any remaining legacy syntax will error at build time — useful for catching stragglers.

## Common migration pitfalls

### `createQueries` signature change

v5's `createQueries` took an array directly. v6 takes an object with a `queries` field (and optional `combine`):

```diff
- const results = createQueries([
-   { queryKey: ['a'], queryFn: fA },
-   { queryKey: ['b'], queryFn: fB },
- ])
+ const results = createQueries(() => ({
+   queries: [
+     { queryKey: ['a'], queryFn: fA },
+     { queryKey: ['b'], queryFn: fB },
+   ],
+ }))
```

### `createMutation` callbacks

The callbacks (`onMutate`, `onSuccess`, etc.) move inside the same options object the accessor returns. They don't need to be functions-of-functions — just put them in the object:

```diff
- const m = createMutation({
-   mutationFn: addTodo,
-   onSuccess: () => invalidate(),
- })
+ const m = createMutation(() => ({
+   mutationFn: addTodo,
+   onSuccess: () => invalidate(),
+ }))
```

### Per-call `mutate` callbacks — unchanged

```ts
// Still works in v6:
mutation.mutate(vars, {
  onSuccess: () => console.log('per-call success'),
})
```

### Reactive `enabled` flag

```diff
- let enabled = writable(false)
+ let enabled = $state(false)

- const query = createQuery(() => ({ enabled: $enabled, /* … */ }))
+ const query = createQuery(() => ({ enabled, /* … */ }))
```

### SSR setup unchanged

The `enabled: browser` pattern, `+layout.ts` `QueryClient` creation, and `prefetchQuery` calls all work the same in v6. No changes needed there.

### Devtools

`@tanstack/svelte-query-devtools` v6 is the version to use. If you were on v5 devtools, upgrade in lockstep:

```bash
npm install @tanstack/svelte-query-devtools@latest
```

### The `isLoading` vs `isPending` distinction

v6 follows the v5 query-core convention:
- `isPending` = `status === 'pending'` (no data yet, regardless of fetching).
- `isLoading` = `isPending && isFetching` (first-ever fetch in flight, no cached data).

If you used `isLoading` in v5 to mean "first load," that still works. If you used it loosely for "fetching," switch to `isPending` (for "no data yet") or `isFetching` (for "request in flight").

### Persisted client plugins

`@tanstack/svelte-query-persist-client` and similar v5 plugins need their v6 counterparts. Update all `@tanstack/svelte-query-*` packages together to v6:

```bash
npm install @tanstack/svelte-query@latest @tanstack/svelte-query-devtools@latest @tanstack/svelte-query-persist-client@latest
```

### Quick codemod script

A rough regex codemod to start from (run on a backup, then review):

```bash
# 1. Wrap options in accessor (manual review needed — won't catch multi-line options)
# Replace:  createQuery\(\s*\{
# With:     createQuery(() => ({
# And the closing: })  →  }))
# This is fragile; a TypeScript AST tool like `jscodeshift` is more reliable.
```

For non-trivial apps, the migration is usually a half-day to a day of mechanical work plus careful testing of reactive flows. The TypeScript compiler is your friend — fix every error it reports and you'll catch most issues.
