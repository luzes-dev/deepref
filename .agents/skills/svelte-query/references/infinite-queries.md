# Infinite queries reference

In-depth reference for `createInfiniteQuery`, `infiniteQueryOptions`, pagination patterns, and bidirectional scrolling. Read this when you need cursor-based or page-based pagination, "load more" buttons, or infinite scroll.

## Table of contents
- createInfiniteQuery
- infiniteQueryOptions helper
- Required options
- Result fields
- Patterns: load-more button
- Patterns: infinite scroll on scroll
- Patterns: bidirectional pagination
- Patterns: cursor vs page-number
- Gotchas

## createInfiniteQuery

```ts
function createInfiniteQuery<
  TQueryFnData = unknown,
  TError = Error,
  TData = InfiniteData<TQueryFnData>,
  TQueryKey extends readonly unknown[] = readonly unknown[],
  TPageParam = unknown
>(
  options: Accessor<CreateInfiniteQueryOptions<TQueryFnData, TError, TData, TQueryKey, TPageParam>>,
  queryClient?: Accessor<QueryClient>
): CreateInfiniteQueryResult<TData, TError>
```

`Accessor<T>` is `() => T`. `TPageParam` is the type of your cursor/page number — could be `number`, `string`, or a complex cursor object.

The result's `data` is `InfiniteData<TQueryFnData>`, which is:

```ts
type InfiniteData<TData, TPageParam = unknown> = {
  pages: TData[]      // one entry per fetched page, in fetch order
  pageParams: TPageParam[]  // the params used to fetch each page
}
```

## infiniteQueryOptions helper

```ts
import { infiniteQueryOptions } from '@tanstack/svelte-query'

export const planetsOptions = infiniteQueryOptions({
  queryKey: ['planets'],
  queryFn: ({ pageParam }) => fetchPlanets(pageParam),
  initialPageParam: 1,
  getNextPageParam: (lastPage) => lastPage.nextPage ?? undefined,
})

// In component:
const query = createInfiniteQuery(() => planetsOptions)

// Or inline:
const query = createInfiniteQuery(() => ({
  queryKey: ['planets'],
  queryFn: ({ pageParam }) => fetchPlanets(pageParam),
  initialPageParam: 1,
  getNextPageParam: (lastPage) => lastPage.nextPage ?? undefined,
}))
```

Use `infiniteQueryOptions` to share the same definition between `createInfiniteQuery` and `queryClient.prefetchInfiniteQuery`.

## Required options

Three options are mandatory on infinite queries:

1. **`queryKey: QueryKey`** — Same as `createQuery`. The cache identity.
2. **`queryFn: (context) => Promise<TQueryFnData>`** — Receives a context with `pageParam: TPageParam`, `queryKey`, `signal`, `meta`. Fetch one page.
3. **`initialPageParam: TPageParam`** — The cursor for the first page. **Required** in v6 (was implicit in v5).
4. **`getNextPageParam: (lastPage, allPages, lastPageParam, allPageParams) => TPageParam | undefined | null`** — Map the last fetched page to the next cursor, or `undefined`/`null` to signal "no more pages".

Optional:
- **`getPreviousPageParam: (firstPage, allPages, firstPageParam, allPageParams) => TPageParam | undefined | null`** — For bidirectional pagination (scroll up to load older).
- **`maxPages: number`** — Cap the number of pages kept in memory. Older pages are dropped from `data.pages`. Useful for memory-bounded infinite scroll.
- All other `createQuery` options apply (`staleTime`, `enabled`, `select`, `retry`, etc.).
- **`select: (data: InfiniteData<TQueryFnData>) => TData`** — Transform the whole `InfiniteData` shape. Useful for flattening pages into a single array the template can iterate.

## Result fields reference

In addition to all the regular `createQuery` fields (`status`, `isPending`, `isError`, `isSuccess`, `isFetching`, `dataUpdatedAt`, etc.), infinite queries expose:

- **`data: InfiniteData<TQueryFnData> | undefined`** — `data.pages` is the array of fetched pages; `data.pageParams` is the matching cursors.
- **`hasNextPage: boolean`** — `true` if `getNextPageParam` returned a non-undefined value for the last page.
- **`hasPreviousPage: boolean`** — `true` if `getPreviousPageParam` returned a non-undefined value for the first page.
- **`isFetchingNextPage: boolean`** — True while fetching the **next** page with `fetchNextPage`.
- **`isFetchingPreviousPage: boolean`** — True while fetching the **previous** page with `fetchPreviousPage`.
- **`fetchNextPage: (options?) => Promise<...>`** — Imperatively fetch the next page. Options: `{ cancelRefetch, throwOnError }`.
- **`fetchPreviousPage: (options?) => Promise<...>`** — Fetch the previous page.

## Patterns: load-more button

```svelte
<script lang="ts">
  import { createInfiniteQuery } from '@tanstack/svelte-query'

  const query = createInfiniteQuery(() => ({
    queryKey: ['planets'],
    queryFn: ({ pageParam = 1 }) =>
      fetch(`https://swapi.dev/api/planets/?page=${pageParam}`).then((r) => r.json()),
    initialPageParam: 1,
    getNextPageParam: (lastPage) => {
      if (!lastPage.next) return undefined
      const url = new URL(lastPage.next)
      return Number(url.searchParams.get('page'))
    },
  }))
</script>

{#if query.isPending}
  Loading...
{:else if query.isError}
  Error: {query.error.message}
{:else}
  {#each query.data.pages as page}
    {#each page.results as planet}
      <p>{planet.name} — population {planet.population}</p>
    {/each}
  {/each}

  <button
    onclick={() => query.fetchNextPage()}
    disabled={!query.hasNextPage || query.isFetchingNextPage}
  >
    {#if query.isFetchingNextPage}
      Loading more...
    {:else if query.hasNextPage}
      Load more
    {:else}
      Nothing more to load
    {/if}
  </button>
{/if}
```

The button states are:
- `isFetchingNextPage` → "Loading more..."
- `hasNextPage && !isFetchingNextPage` → "Load more" (enabled)
- `!hasNextPage` → "Nothing more" (disabled)

## Patterns: infinite scroll on scroll

Use Svelte's `onscroll` (or an `IntersectionObserver` action) to trigger `fetchNextPage` when the user reaches the bottom.

```svelte
<script lang="ts">
  import { createInfiniteQuery } from '@tanstack/svelte-query'

  const query = createInfiniteQuery(() => ({ /* ...same as above... */ }))

  const onScroll = (e: Event) => {
    const el = e.currentTarget as HTMLElement
    const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight
    if (distanceFromBottom < 50 && query.hasNextPage && !query.isFetchingNextPage) {
      query.fetchNextPage()
    }
  }
</script>

<div class="scroll-container" onscroll={onScroll}>
  {#each query.data.pages as page}
    {#each page.results as planet}
      <p>{planet.name}</p>
    {/each}
  {/each}
</div>

<style>
  .scroll-container {
    height: 80vh;
    overflow-y: auto;
  }
</style>
```

Better — use an `IntersectionObserver` sentinel element so you don't pay for scroll-event throttling:

```svelte
<script lang="ts">
  import { onMount } from 'svelte'
  import { createInfiniteQuery } from '@tanstack/svelte-query'

  const query = createInfiniteQuery(() => ({ /* ... */ }))

  let sentinel: HTMLDivElement

  onMount(() => {
    const observer = new IntersectionObserver((entries) => {
      if (entries[0].isIntersecting && query.hasNextPage && !query.isFetchingNextPage) {
        query.fetchNextPage()
      }
    })
    observer.observe(sentinel)
    return () => observer.disconnect()
  })
</script>

{#each query.data.pages as page}
  {#each page.results as planet}
    <p>{planet.name}</p>
  {/each}
{/each}

<div bind:this={sentinel}>
  {#if query.isFetchingNextPage}Loading more...{/if}
</div>
```

## Patterns: bidirectional pagination

For chat-style UIs where you load newest first and scroll up to fetch older messages:

```ts
const messages = createInfiniteQuery(() => ({
  queryKey: ['messages', roomId],
  queryFn: ({ pageParam }) => fetchMessages(roomId, pageParam),
  initialPageParam: initialCursor,
  getPreviousPageParam: (firstPage) => firstPage olderCursor ?? undefined,
  getNextPageParam: (lastPage) => lastPage newerCursor ?? undefined,
}))
```

Use `fetchPreviousPage()` when the user scrolls to the top, `fetchNextPage()` when they reach the bottom. Render pages in order:

```svelte
{#each messages.data.pages as page}
  {#each page.messages as message}
    <Message {message} />
  {/each}
{/each}
```

Note: with `maxPages` set, older pages may be dropped from `data.pages` to bound memory. Be careful with scroll-position restoration — the standard pattern is to capture `scrollHeight` before `fetchPreviousPage` and restore the delta after.

## Patterns: cursor vs page-number

The shape of `TPageParam` is up to you. Two common choices:

**Page number (offset pagination):**
```ts
{
  initialPageParam: 1,
  getNextPageParam: (lastPage, allPages) =>
    lastPage.hasMore ? allPages.length + 1 : undefined,
  queryFn: ({ pageParam }) => fetch(`/api/items?page=${pageParam}`),
}
```

**Opaque cursor (cursor-based, server-issued):**
```ts
{
  initialPageParam: null as string | null,
  getNextPageParam: (lastPage) => lastPage.nextCursor ?? undefined,
  queryFn: ({ pageParam }) =>
    fetch(`/api/items?cursor=${pageParam ?? ''}`),
}
```

Cursor-based is preferred for large/streaming datasets because it's stable under inserts/deletes — a new item inserted at the top doesn't shift page offsets.

## Flattening pages with `select`

If you don't need per-page boundaries in your template, use `select` to flatten:

```ts
const query = createInfiniteQuery(() => ({
  queryKey: ['planets'],
  queryFn: ({ pageParam = 1 }) => fetchPlanets(pageParam),
  initialPageParam: 1,
  getNextPageParam: (lastPage) => lastPage.nextPage,
  select: (data) => ({
    ...data,
    flatItems: data.pages.flatMap((p) => p.results),
  }),
}))

// In template:
{#each query.data.flatItems as planet}{planet.name}{/each}
```

`select` runs after each fetch and is reactive. Keep it pure and cheap.

## Gotchas

- **Don't forget `initialPageParam`.** It's required in v6. If you previously relied on v5's implicit `0`/`1`, set it explicitly.
- **`getNextPageParam` must return `undefined` or `null`** to signal "no more pages" — returning `0` or `''` does not stop pagination (those are valid cursor values).
- **`hasNextPage` is computed from `getNextPageParam(lastPage)`**, not from a separate server field. So `getNextPageParam` returning a truthy cursor means `hasNextPage === true`.
- **`fetchNextPage` is idempotent-ish**: calling it while a fetch is in flight is a no-op (unless you pass `{ cancelRefetch: true }`).
- **`data.pages` order matters.** It's in fetch order — first page is index 0, newest at the end. For bidirectional pagination, prepend-order depends on which direction you're going; the API always appends in fetch order, so pages fetched via `fetchPreviousPage` appear at the start.
- **`select` returning a new object identity every call** can cause unnecessary re-renders. If you only need to derive a list, return a stable structure.
- **With `maxPages`, `data.pageParams.length` shrinks** when pages are dropped. Don't index `pageParams` assuming it includes the initial page forever.
- **`refetchPage` option** (advanced): pass a function `(page, index, allPages) => boolean` to refetch only specific pages when the query is invalidated. Useful for selectively refreshing just the first page.
