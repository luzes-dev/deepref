# Queries reference

In-depth reference for `createQuery`, `createQueries`, `queryOptions`, query keys, options, and result fields. Read this when you need to know which option controls which behavior, or when you're designing query-key shapes for a non-trivial app.

## Table of contents
- createQuery
- createQueries (parallel)
- queryOptions helper
- Query keys
- Options reference
- Result fields reference
- Patterns

## createQuery

```ts
function createQuery<
  TQueryFnData = unknown,
  TError = Error,
  TData = TQueryFnData,
  TQueryKey extends readonly unknown[] = readonly unknown[]
>(
  options: Accessor<UndefinedInitialDataOptions<TQueryFnData, TError, TData, TQueryKey>>,
  queryClient?: Accessor<QueryClient>
): CreateQueryResult<TData, TError>
```

`Accessor<T>` is just `() => T` — the thunk the v6 runes adapter requires. `TQueryFnData` is what your `queryFn` resolves to; `TData` is what the result exposes (defaults to `TQueryFnData`, but can be transformed via `select`).

There are three overloads, picked by which `initialData` shape you pass:

- `UndefinedInitialDataOptions` — `initialData` is absent or a function returning `TQueryFnData | undefined`. Result type is `CreateQueryResult<TData, TError>` where `data` is `TData | undefined`.
- `DefinedInitialDataOptions` — `initialData` is `TQueryFnData | (() => TQueryFnData)`. Result type is `DefinedCreateQueryResult<TData, TError>` where `data` is `TData` (narrowed, non-undefined).
- `CreateQueryOptions` (generic) — the catch-all.

The narrowed type is helpful: when you supply `initialData`, `query.data` is typed as `TData` (not `TData | undefined`) so you can skip the `isPending` branch.

## createQueries (parallel queries)

Use `createQueries` when you need to run multiple independent queries in parallel and combine their results.

```svelte
<script lang="ts">
  import { createQueries } from '@tanstack/svelte-query'

  const results = createQueries(() => ({
    queries: [
      { queryKey: ['user', 1], queryFn: () => fetchUser(1) },
      { queryKey: ['user', 2], queryFn: () => fetchUser(2) },
      { queryKey: ['user', 3], queryFn: () => fetchUser(3) },
    ],
  }))
</script>

{#each results as result}
  {#if result.isSuccess}
    <p>{result.data.name}</p>
  {/if}
{/each}
```

`results` is an array of `CreateQueryResult`s — same shape as `createQuery` returns. To combine into a single value, pass a `combine` function:

```ts
const results = createQueries(() => ({
  queries: [
    { queryKey: ['user', 1], queryFn: () => fetchUser(1) },
    { queryKey: ['user', 2], queryFn: () => fetchUser(2) },
  ],
  combine: (results) => ({
    users: results.map((r) => r.data),
    allLoaded: results.every((r) => r.isSuccess),
    anyPending: results.some((r) => r.isPending),
  }),
}))
// results.users / results.allLoaded / results.anyPending
```

`combine` is reactive — it re-runs whenever any underlying query changes. Be careful returning new object identities each call; this can cause downstream re-renders. Use stable selectors when possible.

Type signature (simplified):
```ts
function createQueries<T extends any[], TCombinedResult = T>(
  createQueriesOptions: Accessor<{
    queries: [...]
    combine?: (results: [...CreateQueryResult]) => TCombinedResult
  }>,
  queryClient?: Accessor<QueryClient>
): TCombinedResult
```

## queryOptions helper

`queryOptions` is a typed factory that lets you define a query's options once and reuse them across components, prefetch calls, and cache reads. It also "locks in" the inferred type, so `queryClient.getQueryData(myOptions.queryKey)` is properly typed.

```ts
import { queryOptions } from '@tanstack/svelte-query'

export const postOptions = (id: number) =>
  queryOptions({
    queryKey: ['posts', id],
    queryFn: () => fetchPost(id),
    staleTime: 60 * 1000,
  })

// Use in component
import { createQuery } from '@tanstack/svelte-query'
const query = createQuery(() => postOptions(props.id))

// Use in +page.ts prefetch
await queryClient.prefetchQuery(postOptions(params.id))

// Use in cache read
const cached = queryClient.getQueryData(postOptions(id).queryKey)
```

`queryOptions` has two overloads (like `createQuery`): one for `DefinedInitialDataOptions` and one for `UndefinedInitialDataOptions`. The return type is `CreateQueryOptions & { queryKey: ... }` — the type system narrows the queryKey to a literal tuple, which is what gives you the typed `getQueryData` calls.

## Query keys

The `queryKey` is the cache identity. Two rules:

1. **Use arrays.** Strings and objects get structurally compared. `['todos', { filter: 'all' }]` is a valid key; `JSON.stringify` of it would also work but arrays let TypeScript narrow tuple types.
2. **Capture every input that affects the fetch.** If `queryFn` reads `filter` and `page`, both go in the key. Otherwise the cache will return stale data when those change.

Common shapes:

```ts
['todos']                              // singleton
['todos', filter]                      // list with a filter
['todos', 'list', { filter, page }]    // namespaced + object params
['todo', id]                           // entity by id
['user', userId, 'posts']              // nested resource
```

Objects in the key are compared by deep equality, so `{ filter: 'all', page: 1 }` matches `{ page: 1, filter: 'all' }`. Don't put functions, class instances, or `Date` objects in keys — they don't serialize reliably.

When you call `queryClient.invalidateQueries({ queryKey: ['todos'] })`, it matches **prefix-wise** — every key that starts with `['todos', ...]` is invalidated. This is why the namespaced shape (`['todos', 'list', ...]` vs `['todo', id]`) is useful: invalidating the list doesn't accidentally invalidate a single entity query that happens to share a prefix.

## Options reference

These are the most-used options on `CreateQueryOptions`. All are optional unless noted.

### Identity & fetching

- **`queryKey: QueryKey`** (required) — Cache identity. Array of strings + serializable values.
- **`queryFn: (context) => Promise<TQueryFnData>`** (required) — The async function that fetches the data. The context has `queryKey`, `signal` (an `AbortSignal` you should pass to `fetch` so cancellation works), `meta`, and `pageParam` (for infinite queries).
- **`enabled: boolean`** — When `false`, the query won't auto-run. Useful for dependent queries (`enabled: !!userId`). You can still call `query.refetch()` manually.
- **`initialData: TQueryFnData | (() => TQueryFnData | undefined)`** — Pre-seeds the cache. The query starts in `success` state with this data and refetches in the background if stale. Use for SSR or to bootstrap from props.
- **`placeholderData: TQueryFnData | ((prev: TQueryFnData | undefined) => TQueryFnData | undefined)`** — Like `initialData` but doesn't persist; useful for showing the previous query's data while a new one loads (`placeholderData: (prev) => prev` keeps the previous data visible).
- **`select: (data: TQueryFnData) => TData`** — Transform or filter the cached data before it hits the component. Great for picking one field out of a larger response.

### Timing

- **`staleTime: number`** (ms, default `0`) — How long data is considered fresh. While fresh, no refetches happen on mount, window focus, or `invalidateQueries` (without `refetchType: 'active'`). Default `0` means "always refetch on mount"; bump it to 30–60s for typical list data.
- **`gcTime: number`** (ms, default `5 * 60 * 1000`) — formerly `cacheTime`. How long an unused query stays in memory before garbage collection.
- **`refetchInterval: number | false | ((query) => number | false)`** — Polling interval. Set to a number for fixed-rate polling; use the function form for adaptive intervals.
- **`refetchIntervalInBackground: boolean`** — Keep polling even when the tab is hidden (default `false`).
- **`refetchOnWindowFocus: boolean | 'always'`** — Refetch when the window regains focus. Default `true`. `'always'` ignores `staleTime`.
- **`refetchOnMount: boolean | 'always'`** — Refetch when the component mounts. Default `true` (but only if stale).
- **`refetchOnReconnect: boolean | 'always'`** — Refetch when network reconnects.

### Retry & error handling

- **`retry: number | boolean | ((failureCount, error) => boolean)`** (default `3`) — Number of retry attempts on failure. `false` to disable. Function form lets you decide based on the error (e.g., don't retry 4xx).
- **`retryDelay: (attempt) => number`** — Backoff function. Default is exponential: `Math.min(1000 * 2 ** attempt, 30000)`.
- **`throwOnError: boolean | ((error, query) => boolean)`** — If `true`, errors are thrown during render so a Svelte error boundary can catch them (default `false`, errors land in `query.error`).

### Network behavior

- **`networkMode: 'online' | 'always' | 'offlineFirst'`** (default `'online'`) — `'online'` pauses fetches when offline; `'always'` retries even offline; `'offlineFirst'` tries once even offline then pauses.

## Result fields reference

The object returned by `createQuery` is reactive — read fields directly in templates, no `$` prefix.

### Status

- **`status: 'pending' | 'error' | 'success'`** — Discriminated union; branch on this for clarity.
- **`fetchStatus: 'fetching' | 'paused' | 'idle'`** — Network-level status. `'paused'` means waiting for reconnect (`networkMode: 'online'`). `fetchStatus` and `status` are independent: a query can be `status: 'success'` and `fetchStatus: 'fetching'` during a background refetch.
- **`isPending: boolean`** — `status === 'pending'` and no data yet.
- **`isError: boolean`** — `status === 'error'`.
- **`isSuccess: boolean`** — `status === 'success'`.
- **`isLoading: boolean`** — `isPending && isFetching`. True only on the first-ever fetch when there's no cached data. Often confused with `isPending`; usually `isPending` is what you want.
- **`isFetching: boolean`** — `fetchStatus === 'fetching'`. True during any in-flight request, including background refetches.
- **`isPaused: boolean`** — `fetchStatus === 'paused'`.

### Data & error

- **`data: TData | undefined`** — The current data. `undefined` unless `status === 'success'` (or `initialData` was provided).
- **`error: TError | null`** — The current error. `null` unless `status === 'error'`.
- **`dataUpdatedAt: number`** — Timestamp (ms since epoch) of the last successful fetch. Useful for "Updated 5s ago" displays and for deciding whether to refetch.
- **`errorUpdatedAt: number`** — Timestamp of the last error.
- **`failureCount: number`** — How many fetches have failed in a row. Reset on success.
- **`failureReason: TError | null`** — The most recent failure's error.
- **`isStale: boolean`** — Whether the cached data is past `staleTime`.

### Methods

- **`refetch: () => Promise<QueryObserverResult>`** — Imperatively refetch. Resolves with the new result (or throws).
- **`fetchMore` / `fetchNextPage` / `fetchPreviousPage`** — Only on `createInfiniteQuery` results.

## Patterns

### Dependent queries

```ts
const user = createQuery(() => ({
  queryKey: ['user', userId()],
  queryFn: fetchUser,
  enabled: !!userId(),
}))

// Wait for user before fetching their posts
const posts = createQuery(() => ({
  queryKey: ['posts', user.data?.id],
  queryFn: () => fetchPosts(user.data!.id),
  enabled: !!user.data?.id,
}))
```

### Polling

```ts
const query = createQuery(() => ({
  queryKey: ['notifications'],
  queryFn: fetchNotifications,
  refetchInterval: 5000,           // every 5s
  refetchIntervalInBackground: false, // pause when tab hidden
}))
```

### Selecting derived data

```ts
const query = createQuery(() => ({
  queryKey: ['posts'],
  queryFn: fetchPosts,
  select: (posts) => posts.filter((p) => !p.draft),
}))
// query.data is now Post[] of published only
```

### Placeholder data (keep previous while loading next)

```ts
import { keepPreviousData } from '@tanstack/svelte-query'

const query = createQuery(() => ({
  queryKey: ['posts', page],
  queryFn: () => fetchPosts(page),
  placeholderData: keepPreviousData,  // shows previous page's data while new page loads
}))
```

### Disabling a query temporarily

```ts
const query = createQuery(() => ({
  queryKey: ['expensive', inputs()],
  queryFn: () => expensiveComputation(inputs()),
  enabled: inputsValid(),
}))
```
