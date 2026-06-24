# SSR and SvelteKit reference

How to use svelte-query with SvelteKit's server-side rendering, prefetching, and hydration. Read this when the app is on SvelteKit (not plain Svelte) or when you need to prefetch data before navigation.

## Table of contents
- The SSR gotcha: `enabled: browser`
- Strategy 1: pass `initialData` from `load`
- Strategy 2: prefetch in `+page.ts` (recommended)
- Strategy 3: `HydrationBoundary` for dehydrated state
- Full SvelteKit setup
- Gotchas

## The SSR gotcha: `enabled: browser`

SvelteKit renders routes with SSR by default. If you let `createQuery` run on the server, the query keeps fetching **after** HTML is sent to the client — the server has no signal "we're done, stop fetching." This wastes resources and can throw unhandled-rejection errors on the server.

The fix is to disable query auto-fetching on the server:

```svelte
<!-- src/routes/+layout.svelte -->
<script lang="ts">
  import { browser } from '$app/environment'
  import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query'

  const queryClient = new QueryClient({
    defaultOptions: {
      queries: {
        enabled: browser,  // only run queries in the browser
      },
    },
  })
</script>

<QueryClientProvider client={queryClient}>
  {@render children()}
</QueryClientProvider>
```

This does **not** disable `queryClient.prefetchQuery()` — that's an explicit imperative call, which we use during SSR load functions. So with `enabled: browser`, the pattern becomes: prefetch manually in `+page.ts`, then `createQuery` in `+page.svelte` finds the cache already populated.

## Strategy 1: pass `initialData` from `load`

Simplest approach — fetch in the `load` function, pass the result as `initialData` to `createQuery`.

```ts
// src/routes/+page.ts
export async function load() {
  const posts = await getPosts()
  return { posts }
}
```

```svelte
<!-- src/routes/+page.svelte -->
<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import type { PageData } from './$types'

  const { data } = $props<PageData>()

  const query = createQuery(() => ({
    queryKey: ['posts'],
    queryFn: getPosts,
    initialData: data.posts,
  }))
</script>
```

Pros:
- Minimal setup — one option.
- Works with `+page.ts`, `+page.server.ts`, `+layout.ts`, `+layout.server.ts`.

Cons:
- You must prop-drill `initialData` to whichever component calls `createQuery`. If that component is deep in the tree, it's painful.
- If multiple components call `createQuery(['posts'])`, each needs the same `initialData`.
- `dataUpdatedAt` is the page-load time, not the actual fetch time. The query is treated as stale immediately, so a refetch fires on mount. Usually fine, sometimes wasteful.

## Strategy 2: prefetch in `+page.ts` (recommended)

The canonical pattern for SvelteKit + svelte-query. Create the `QueryClient` in a `+layout.ts` load function, prefetch queries in `+page.ts` load functions, and consume the cached data in components.

```ts
// src/routes/+layout.ts
import { browser } from '$app/environment'
import { QueryClient } from '@tanstack/svelte-query'
import type { LayoutLoad } from './$types'

export const load: LayoutLoad = () => {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: {
        enabled: browser,
        staleTime: 60 * 1000,  // optional but recommended
      },
    },
  })
  return { queryClient }
}
```

```svelte
<!-- src/routes/+layout.svelte -->
<script lang="ts">
  import { QueryClientProvider } from '@tanstack/svelte-query'
  import type { LayoutData } from './$types'

  const { data, children } = $props<{ data: LayoutData }>()
</script>

<QueryClientProvider client={data.queryClient}>
  {@render children()}
</QueryClientProvider>
```

```ts
// src/routes/+page.ts
import type { PageLoad } from './$types'

export const load: PageLoad = async ({ parent, fetch }) => {
  const { queryClient } = await parent()

  // IMPORTANT: use SvelteKit's `fetch` (the second arg) so relative URLs
  // work on both server and client, and so SvelteKit can track deps.
  await queryClient.prefetchQuery({
    queryKey: ['posts', 10],
    queryFn: async () => (await fetch('/api/posts')).json(),
  })
}
```

```svelte
<!-- src/routes/+page.svelte -->
<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'

  const query = createQuery(() => ({
    queryKey: ['posts', 10],
    queryFn: async () => (await fetch('/api/posts')).json(),
  }))
  // Because +page.ts prefetched this exact key, the cache is already populated.
  // query.data is defined on first render (no loading flash).
</script>
```

Pros:
- Server-loaded data is available anywhere — no prop drilling.
- No initial client-side fetch on first render.
- `dataUpdatedAt` reflects when the server prefetched, so `staleTime` works correctly (the query stays fresh for `staleTime` ms after server-render).
- Multiple components can read the same key without each needing `initialData`.

Cons:
- More files (`+layout.ts`, `+page.ts`, `+layout.svelte`, `+page.svelte`).
- Won't work with `+page.server.ts`/`+layout.server.ts` — APIs called by TanStack Query need to be reachable from the browser anyway, so use `+page.ts`/`+layout.ts` instead. If you need server-only secrets, expose them via a SvelteKit `+server.ts` endpoint and call that endpoint from the queryFn.

### Prefetching multiple queries in parallel

```ts
export const load = async ({ parent, fetch }) => {
  const { queryClient } = await parent()

  await Promise.all([
    queryClient.prefetchQuery({
      queryKey: ['posts'],
      queryFn: async () => (await fetch('/api/posts')).json(),
    }),
    queryClient.prefetchQuery({
      queryKey: ['user', 'me'],
      queryFn: async () => (await fetch('/api/me')).json(),
    }),
  ])
}
```

### Using `queryOptions` to share definitions

```ts
// src/lib/queries.ts
import { queryOptions } from '@tanstack/svelte-query'

export const postsQuery = (limit: number) =>
  queryOptions({
    queryKey: ['posts', limit],
    queryFn: async () => (await fetch(`/api/posts?limit=${limit}`)).json(),
    staleTime: 60_000,
  })
```

```ts
// src/routes/+page.ts
import { postsQuery } from '$lib/queries'
export const load = async ({ parent }) => {
  const { queryClient } = await parent()
  await queryClient.prefetchQuery(postsQuery(10))
}
```

```svelte
<!-- src/routes/+page.svelte -->
<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { postsQuery } from '$lib/queries'
  const query = createQuery(() => postsQuery(10))
</script>
```

This is the cleanest pattern — the query definition lives in one place, and `+page.ts` (prefetch) and `+page.svelte` (subscribe) reference the same factory.

## Strategy 3: `HydrationBoundary` for dehydrated state

For more complex SSR scenarios (e.g., you're using a custom server, or you want to share dehydrated state across multiple clients), use the `HydrationBoundary` component.

```ts
// +layout.ts — dehydrate the client on the server
import { dehydrate } from '@tanstack/svelte-query'

export const load = async () => {
  const queryClient = new QueryClient({ /* ... */ })
  await queryClient.prefetchQuery(/* ... */)
  return {
    dehydratedState: dehydrate(queryClient),
  }
}
```

```svelte
<!-- +layout.svelte -->
<script lang="ts">
  import { QueryClient, HydrationBoundary } from '@tanstack/svelte-query'
  const { data } = $props()

  const queryClient = new QueryClient()
</script>

<QueryClientProvider client={queryClient}>
  <HydrationBoundary state={data.dehydratedState}>
    {@render children()}
  </HydrationBoundary>
</QueryClientProvider>
```

`HydrationBoundary` reads the `state` prop and writes it into the `QueryClient`'s cache on mount. Use this when you want a fresh `QueryClient` per browser session but want to seed it with server-fetched data.

For most SvelteKit apps, Strategy 2 (prefetch in `+page.ts`) is enough. Reach for `HydrationBoundary` when you have a more involved setup or want to hydrate non-prefetched queries.

## Full SvelteKit setup (recommended starter)

```
src/
├── lib/
│   └── queries.ts              ← shared queryOptions factories
├── routes/
│   ├── +layout.ts              ← creates QueryClient, returns it
│   ├── +layout.svelte          ← wraps app in QueryClientProvider
│   ├── +page.ts                ← prefetches per-route queries
│   ├── +page.svelte            ← calls createQuery with same keys
│   └── [postId]/
│       ├── +page.ts            ← prefetches detail query
│       └── +page.svelte        ← reads detail query
└── app.html
```

### `+layout.ts`

```ts
import { browser } from '$app/environment'
import { QueryClient } from '@tanstack/svelte-query'
import type { LayoutLoad } from './$types'

export const load: LayoutLoad = () => {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: {
        enabled: browser,
        staleTime: 60_000,
      },
    },
  })
  return { queryClient }
}
```

### `+layout.svelte`

```svelte
<script lang="ts">
  import '../app.css'
  import { QueryClientProvider } from '@tanstack/svelte-query'
  import { SvelteQueryDevtools } from '@tanstack/svelte-query-devtools'

  const { data, children } = $props()
</script>

<QueryClientProvider client={data.queryClient}>
  <main>
    {@render children()}
  </main>
  {#if browser}
    <SvelteQueryDevtools />
  {/if}
</QueryClientProvider>
```

### `+page.ts`

```ts
import type { PageLoad } from './$types'
import { postsQuery } from '$lib/queries'

export const load: PageLoad = async ({ parent, fetch }) => {
  const { queryClient } = await parent()
  await queryClient.prefetchQuery(postsQuery(10))
}
```

### `+page.svelte`

```svelte
<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { postsQuery } from '$lib/queries'

  const query = createQuery(() => postsQuery(10))
</script>

{#if query.isPending}
  Loading…
{:else if query.isError}
  Error: {query.error.message}
{:else}
  {#each query.data as post}
    <a href={`/${post.id}`}>{post.title}</a>
  {/each}
{/if}
```

## Gotchas

- **`enabled: browser` is the single most important setting.** Without it, queries run on the server, never resolve (the response is already sent), and you get unhandled-promise warnings.
- **Always use SvelteKit's `fetch` in `+page.ts`/`+layout.ts`** (the second argument to `load`), not global `fetch`. SvelteKit's `fetch` handles relative URLs on the server and lets SvelteKit track dependencies for invalidation.
- **`staleTime` matters more on SSR.** With `staleTime: 0` (default), the browser refetches the moment the page hydrates, defeating the point of prefetching. Set a sensible `staleTime` (30s–5min depending on data volatility) in your default options or per-query via `queryOptions`.
- **Don't create the `QueryClient` at module scope.** A module-scope client is shared across requests — one user's cached data leaks to another. Always create it inside the `load` function.
- **Browser-only APIs in `queryFn`:** if your `queryFn` touches `window` or `localStorage`, guard with `if (!browser) return` or use `enabled: browser` so it never runs server-side.
- **`+page.server.ts` vs `+page.ts`:** TanStack Query fetches happen in the browser too (for refetches), so the API needs to be reachable from the browser. Use `+page.ts`, not `+page.server.ts`, for prefetching. If you need server-only secrets, put them behind a `+server.ts` endpoint.
- **Devtools on the server:** `SvelteQueryDevtools` may warn when SSR'd. Wrap in `{#if browser}<SvelteQueryDevtools />{/if}` or only import it in dev.
- **Custom `transformPageProps`:** if you customize SvelteKit's page data shape, make sure `queryClient` flows through. The layout's `load` returns it; pages get it via `await parent()`.
- **Persistent client across navigation:** because the `QueryClient` is created in `+layout.ts` and returned from `load`, SvelteKit gives you the **same** instance on the client after hydration (no client-only recreation), but a **new** instance per server request. That's exactly what you want.
