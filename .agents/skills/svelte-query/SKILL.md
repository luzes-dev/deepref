---
name: svelte-query
description: Use TanStack Query (svelte-query) for server-state management in Svelte 5 / SvelteKit apps — fetching, caching, mutations, optimistic updates, infinite scroll, and SSR. Trigger this skill whenever the user is building a Svelte or SvelteKit app and mentions `@tanstack/svelte-query`, `createQuery`, `createMutation`, `createInfiniteQuery`, `QueryClient`, `QueryClientProvider`, "svelte-query", "TanStack Query with Svelte", data fetching with caching in Svelte, optimistic UI updates in Svelte, paginated/infinite lists in Svelte, prefetching data in SvelteKit, or server state in Svelte 5 runes. Also trigger when the user is migrating a Svelte app from svelte-query v5 (stores) to v6 (runes), or asks "how do I use TanStack Query in Svelte 5?". Do NOT trigger for plain React Query (`@tanstack/react-query`) or non-Svelte TanStack Query usage.
---

# svelte-query

TanStack Query (`@tanstack/svelte-query`) is the Svelte 5 adapter for TanStack Query — a library for managing **server state** in client apps. It handles fetching, caching, synchronizing, and updating asynchronous data so you don't have to hand-roll loading flags, error states, and refetch logic.

This skill covers **v6**, which uses Svelte 5 runes under the hood (no more `$` store prefixes). If the user is on v5 or earlier stores syntax, see `references/migration-v5-to-v6.md`.

## Mental model

Three ideas drive everything in svelte-query:

1. **Queries read server state.** `createQuery` registers a `queryKey` + `queryFn` pair with a shared `QueryClient`. The client caches the result keyed by `queryKey`, dedupes concurrent fetches, refetches in the background when data goes stale, and exposes a reactive result object you can read in your template.

2. **Mutations write server state.** `createMutation` wraps a side-effecting function (`mutationFn`) and exposes `mutate(...)` / `mutateAsync(...)`. After a mutation you typically call `queryClient.invalidateQueries({ queryKey })` so the affected queries refetch — this is the canonical "tell the cache the world changed" pattern.

3. **The cache is the source of truth.** Components don't fetch — they ask the cache for data, and the cache decides whether to fetch, refetch, or return instantly. This is why two components reading the same `queryKey` get the same data with one network round-trip.

## The one rule that breaks everyone: the accessor pattern

Every `create*` function in svelte-query takes its options wrapped in a **thunk** — a function that returns the options object:

```svelte
<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'

  // CORRECT: arrow function returning the options object
  const query = createQuery(() => ({
    queryKey: ['todos'],
    queryFn: () => fetchTodos(),
  }))
</script>
```

Why? Svelte 5 runes need to track which `$state`/`$props` values flow into the options so they can trigger refetches when those values change. Passing a plain object once would freeze the inputs. The thunk is re-invoked whenever its reactive dependencies change, so:

```svelte
<script lang="ts">
  let filter = $state('all')              // reactive
  const query = createQuery(() => ({
    queryKey: ['todos', filter],           // changes when filter changes -> refetch
    queryFn: () => fetchTodos(filter),
  }))
</script>
```

If you forget the wrapper, TypeScript will complain, but worth internalizing: **options are always `() => ({...})`, never `{...}`**.

Also: because v6 uses runes, **do not prefix the result with `$`**. Write `query.data`, not `$query.data`.

## Install

```bash
npm i @tanstack/svelte-query
# or: pnpm add @tanstack/svelte-query / yarn add / bun add
```

Requires **Svelte ≥ 5.25.0**. v6 depends on `@tanstack/query-core` v5.

## Setup: provide a QueryClient

Wrap the root of your app in `QueryClientProvider`. The client holds the cache and must live for the lifetime of the app.

```svelte
<script lang="ts">
  import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query'
  import Example from './lib/Example.svelte'

  const queryClient = new QueryClient()
</script>

<QueryClientProvider client={queryClient}>
  <Example />
</QueryClientProvider>
```

For SvelteKit (SSR), see `references/ssr.md` — you need `enabled: browser` to stop queries running on the server after HTML is sent.

## Read a query

```svelte
<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'

  const query = createQuery(() => ({
    queryKey: ['todos'],
    queryFn: () => fetch('/api/todos').then((r) => r.json()),
  }))
</script>

{#if query.isPending}
  Loading...
{:else if query.isError}
  Error: {query.error.message}
{:else if query.isSuccess}
  {#each query.data as todo}
    <p>{todo.title}</p>
  {/each}
{/if}

{#if query.isFetching}
  <span>Background refreshing…</span>
{/if}
```

The result exposes both a discriminated `status` (`'pending' | 'error' | 'success'`) and convenience flags (`isPending`, `isError`, `isSuccess`, `isFetching`, `isLoading`). Use whichever reads best in context. `isFetching` is true whenever a request is in flight — including background refetches — so it's the right flag for "refreshing…" spinners that should appear even after data is shown.

`queryKey` is everything that identifies the data. Use an array of strings + values; serialize objects so they compare structurally (`['todos', { filter, page }]`). Two components using the same key share one cache entry.

## Run a mutation

```svelte
<script lang="ts">
  import { createMutation, useQueryClient } from '@tanstack/svelte-query'

  const queryClient = useQueryClient()

  const addTodo = createMutation(() => ({
    mutationFn: (text: string) =>
      fetch('/api/todos', { method: 'POST', body: JSON.stringify({ text }) })
        .then((r) => r.json()),
    onSuccess: () => {
      // tell the cache the todos list is stale -> refetch
      queryClient.invalidateQueries({ queryKey: ['todos'] })
    },
  }))
</script>

<form onsubmit={(e) => { e.preventDefault(); addTodo.mutate(text) }}>
  <input bind:value={text} />
  <button disabled={addTodo.isPending}>Add</button>
</form>

{#if addTodo.isError}
  Failed: {addTodo.error.message}
{/if}
```

- Call `addTodo.mutate(variables)` for fire-and-forget; use `addTodo.mutateAsync(variables)` if you need to `await` it.
- Per-call callbacks: `addTodo.mutate(vars, { onSuccess, onError, onSettled })`.
- Optimistic updates (show the change before the server confirms it) are a common pattern — see `references/mutations.md`.

## Infinite / paginated queries

```svelte
<script lang="ts">
  import { createInfiniteQuery } from '@tanstack/svelte-query'

  const query = createInfiniteQuery(() => ({
    queryKey: ['planets'],
    queryFn: ({ pageParam }) => fetch(`/api/planets?page=${pageParam}`).then((r) => r.json()),
    initialPageParam: 1,
    getNextPageParam: (lastPage) => lastPage.next ? lastPage.nextPage : undefined,
  }))
</script>

{#each query.data.pages as page}
  {#each page.results as planet}
    <p>{planet.name}</p>
  {/each}
{/each}

<button
  onclick={() => query.fetchNextPage()}
  disabled={!query.hasNextPage || query.isFetchingNextPage}
>
  {query.isFetchingNextPage ? 'Loading…' : 'Load more'}
</button>
```

`createInfiniteQuery` requires `initialPageParam` and a `getNextPageParam` (and optionally `getPreviousPageParam`) function that maps a page to the next cursor, or `undefined` if there's no more. The result exposes `data.pages` (array of pages) and `data.pageParams`. See `references/infinite-queries.md` for bi-directional and bidirectional patterns.

## Reading the cache directly

```svelte
<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  const qc = useQueryClient()

  // read (returns undefined if not cached)
  const todo = qc.getQueryData(['todo', 1])

  // write (skips the queryFn, just updates cache + notifies subscribers)
  qc.setQueryData(['todo', 1], updatedTodo)

  // invalidate (marks stale and refetches if active)
  qc.invalidateQueries({ queryKey: ['todos'] })

  // prefetch (load before navigation, don't subscribe)
  await qc.prefetchQuery({ queryKey: ['todo', 2], queryFn: () => fetchTodo(2) })
</script>
```

`useQueryClient()` returns the nearest `QueryClient` from context. Use it for cache reads/writes and invalidations from inside components. For SSR prefetching in SvelteKit `load` functions, see `references/ssr.md`.

## Available API surface

Functions and components exported by `@tanstack/svelte-query`:

| Export | Purpose |
|---|---|
| `createQuery` | Read a single query (most common) |
| `createQueries` | Run multiple queries in parallel, optionally combine results |
| `createInfiniteQuery` | Paginated / cursor-based infinite lists |
| `createMutation` | Run a side-effecting operation; mutate / mutateAsync |
| `queryOptions` | Helper to build a typed `CreateQueryOptions` object (great for sharing between `prefetchQuery` and `createQuery`) |
| `infiniteQueryOptions` | Same helper for infinite queries |
| `mutationOptions` | Same helper for mutations |
| `useQueryClient` | Get the `QueryClient` from context |
| `useIsFetching` | Reactive count of in-flight queries (filtered) |
| `useIsMutating` | Reactive count of in-flight mutations (filtered) |
| `useMutationState` | Subscribe to mutation cache state |
| `useIsRestoring` | True while a `HydrationBoundary` is restoring cache |
| `useHydrate` | Imperatively hydrate a dehydrated state |
| `QueryClient` | The cache-holding class you instantiate once |
| `QueryClientProvider` | Svelte component that puts the client on context |
| `HydrationBoundary` | Component that hydrates a dehydrated state into the client (for SSR) |
| `getQueryClientContext` / `setQueryClientContext` | Low-level context access (rarely needed directly) |
| `getIsRestoringContext` / `setIsRestoringContext` | Low-level context access (rarely needed directly) |

For full signatures and type parameters, see `references/api-reference.md`.

## Common pitfalls

- **Forgetting the `() => ({...})` wrapper.** Without it, options won't be reactive and TypeScript will reject the call.
- **Prefixing result properties with `$`.** That was the v5 stores syntax. In v6 runes, `query.data` — not `$query.data`.
- **Running queries on the server in SvelteKit.** The query keeps fetching after HTML is sent. Set `defaultOptions: { queries: { enabled: browser } }` on your `QueryClient`. See `references/ssr.md`.
- **Using `isLoading` when you mean `isPending`.** `isLoading` is shorthand for `isPending && isFetching` — true only on the first ever fetch with no cached data. For "show a spinner on first load", `isPending` is usually what you want.
- **Forgetting to invalidate after a mutation.** The cache won't know your server state changed; the UI will look stale. Always pair `createMutation`'s `onSuccess` (or `onSettled`) with `queryClient.invalidateQueries`.
- **Building query keys inconsistently.** `['todos']` and `['todos', undefined]` are different keys. Pick a shape and stick to it; the `queryOptions` helper encourages reuse.
- **Naming collisions with `each` blocks.** `query.data.pages` in an infinite query — make sure you `{#each query.data.pages as page}` and read fields off `page`, not off `query.data`.
- **Mutations without `mutationKey`.** Only required if you want to use `useMutationState` or `useIsMutating` filters — but adding one from the start is cheap.

## When to reach for which reference

- `references/queries.md` — Full list of query options (`staleTime`, `gcTime`, `enabled`, `refetchInterval`, `select`, `placeholderData`, retry config, etc.) and result fields; `queryOptions` helper; `createQueries` for parallel queries; query-key design.
- `references/mutations.md` — Mutation options and lifecycle callbacks; `mutate` vs `mutateAsync`; per-call callbacks; optimistic updates with rollback; invalidation strategies; `useMutationState`.
- `references/infinite-queries.md` — `createInfiniteQuery` options; bidirectional pagination; `infiniteQueryOptions` helper; "load more" and infinite scroll UX.
- `references/ssr.md` — SvelteKit SSR setup, `prefetchQuery` in `+page.ts`, `initialData` vs `prefetchQuery`, `HydrationBoundary`, `browser` flag.
- `references/devtools.md` — `SvelteQueryDevtools` install, mount, options (`initialIsOpen`, `buttonPosition`, `position`, `client`, `errorTypes`, `styleNonce`, `shadowDOMTarget`).
- `references/migration-v5-to-v6.md` — Converting stores syntax (`$query.data`, `derived`, `writable`) to runes (`$state`, accessor pattern). Required reading if upgrading an existing v5 app.
- `references/api-reference.md` — Full function signatures, type parameters, and return types. Consult when you need exact types or for TypeScript-heavy work.

## Copyable example templates

The `examples/` folder has ready-to-paste Svelte components:

- `examples/basic-query.svelte` — `createQuery` with loading/error/success branches
- `examples/mutation.svelte` — `createMutation` + `invalidateQueries` after success
- `examples/optimistic-updates.svelte` — full `onMutate` / `onError` / `onSettled` rollback pattern
- `examples/infinite-scroll.svelte` — `createInfiniteQuery` with a "Load more" button
- `examples/auto-refetch.svelte` — `refetchInterval` driven by reactive `$state`
- `examples/ssr/+layout.svelte`, `examples/ssr/+layout.ts`, `examples/ssr/+page.ts`, `examples/ssr/+page.svelte` — full SvelteKit SSR prefetch setup

## Workflow: building a feature with svelte-query

1. Identify the data: what entity, which params go in the `queryKey`.
2. Write a plain `async function` that fetches it (no svelte-query yet).
3. Decide if it's a query (read) or mutation (write). Usually both — list + add/edit/delete.
4. For each query, pick a `queryKey` shape that captures every input. Use `queryOptions()` to define it once and share between prefetch and component code.
5. In the component, call `createQuery(() => options())` and branch the template on `status` (or the `is*` flags).
6. For mutations, wire `onSuccess` to `queryClient.invalidateQueries({ queryKey })` so the related list refetches.
7. Add `<SvelteQueryDevtools />` during development so you can see cache state live.
8. If on SvelteKit, set up SSR prefetching in `+page.ts` via `queryClient.prefetchQuery` and pass the client through layout context. See `references/ssr.md`.

## TypeScript

All functions are heavily generically typed. The most useful pattern is to type your fetcher once and let it flow:

```ts
import { queryOptions } from '@tanstack/svelte-query'

type Todo = { id: number; title: string; done: boolean }

export const todoOptions = (id: number) =>
  queryOptions<Todo>({
    queryKey: ['todos', id],
    queryFn: () => fetch(`/api/todos/${id}`).then((r) => r.json()),
  })

// Then in a component:
import { createQuery } from '@tanstack/svelte-query'
const query = createQuery(() => todoOptions(props.id))
// query.data is typed as Todo | undefined
```

For an exhaustive type list (type params, option types, result types) see `references/api-reference.md`.

## Quick rules of thumb

- **Wrap options in `() => ({...})`**. Always. No exceptions.
- **Invalidate after mutations**. `onSuccess: () => qc.invalidateQueries({ queryKey })`.
- **Use the same `queryKey` for shared data**. Two components, same key, one fetch.
- **`isPending`** for first-load spinners, **`isFetching`** for background-refresh spinners.
- **In SvelteKit, set `enabled: browser`** in the default query options to avoid server-side fetches.
- **Read `$state`/`$props` directly inside the accessor** — no `$derived` wrapping needed.
- **Add `SvelteQueryDevtools` in dev** — it's the fastest way to debug cache state.
