# API reference

Full function signatures, type parameters, and return types for everything exported by `@tanstack/svelte-query`. Use this when you need exact types for TypeScript-heavy work or are unsure which overload you're hitting.

## Table of contents
- Types & conventions
- Functions
  - createQuery
  - createQueries
  - createInfiniteQuery
  - createMutation
  - queryOptions
  - infiniteQueryOptions
  - mutationOptions
  - useQueryClient
  - useIsFetching
  - useIsMutating
  - useMutationState
  - useIsRestoring
  - useHydrate
  - getQueryClientContext / setQueryClientContext
  - getIsRestoringContext / setIsRestoringContext
- Components
  - QueryClientProvider
  - HydrationBoundary
- Type aliases

## Types & conventions

### `Accessor<T>`

```ts
type Accessor<T> = () => T
```

The thunk wrapper that v6 requires on every `create*` function's options argument. Wraps the options object in a function so the runes adapter can track reactive dependencies.

### Reactive values

Several `use*` hooks return `ReactiveValue<T>` (a Svelte 5 reactive wrapper). In templates they auto-unwrap; in script, read `.value`:

```ts
const fetching = useIsFetching()
console.log(fetching.value)  // number
```

The result of `createQuery` / `createMutation` / `createInfiniteQuery` is reactive but accessed via plain property reads (no `.value`, no `$` prefix) — the runes adapter unwraps internally.

---

## Functions

### `createQuery`

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

function createQuery<...>(
  options: Accessor<DefinedInitialDataOptions<TQueryFnData, TError, TData, TQueryKey>>,
  queryClient?: Accessor<QueryClient>
): DefinedCreateQueryResult<TData, TError>

function createQuery<...>(
  options: Accessor<CreateQueryOptions<TQueryFnData, TError, TData, TQueryKey>>,
  queryClient?: Accessor<QueryClient>
): CreateQueryResult<TData, TError>
```

Three overloads, picked by which `initialData` shape is passed. `DefinedInitialDataOptions` narrows the return type so `data` is `TData` (not `TData | undefined`).

### `createQueries`

```ts
function createQueries<
  T extends any[],
  TCombinedResult = /* inferred from `combine` */
>(
  createQueriesOptions: Accessor<{
    queries: readonly [CreateQueryOptionsForCreateQueries<Head>, ...]
    combine?: (results: [...CreateQueryResult]) => TCombinedResult
  }>,
  queryClient?: Accessor<QueryClient>
): TCombinedResult
```

Returns the array of results, or — if `combine` is provided — the value returned by `combine`. The `T` type parameter is the tuple of input option types, used to infer the per-result types in the output array.

### `createInfiniteQuery`

```ts
function createInfiniteQuery<
  TQueryFnData,
  TError = Error,
  TData = InfiniteData<TQueryFnData>,
  TQueryKey extends readonly unknown[] = readonly unknown[],
  TPageParam = unknown
>(
  options: Accessor<CreateInfiniteQueryOptions<TQueryFnData, TError, TData, TQueryKey, TPageParam>>,
  queryClient?: Accessor<QueryClient>
): CreateInfiniteQueryResult<TData, TError>
```

### `createMutation`

```ts
function createMutation<
  TData = unknown,
  TError = Error,
  TVariables = void,
  TContext = unknown
>(
  options: Accessor<CreateMutationOptions<TData, TError, TVariables, TContext>>,
  queryClient?: Accessor<QueryClient>
): CreateMutationResult<TData, TError, TVariables, TContext>
```

### `queryOptions`

```ts
function queryOptions<
  TQueryFnData = unknown,
  TError = Error,
  TData = TQueryFnData,
  TQueryKey extends readonly unknown[] = readonly unknown[]
>(
  options: DefinedInitialDataOptions<TQueryFnData, TError, TData, TQueryKey>
): CreateQueryOptions<TQueryFnData, TError, TData, TQueryKey> & {
  queryKey: TQueryKey
  initialData: NonUndefinedGuard<TQueryFnData> | (() => NonUndefinedGuard<TQueryFnData>)
}

function queryOptions<...>(
  options: UndefinedInitialDataOptions<TQueryFnData, TError, TData, TQueryKey>
): CreateQueryOptions<TQueryFnData, TError, TData, TQueryKey> & {
  queryKey: TQueryKey
}
```

Returns the same options you passed in, but with `queryKey` narrowed to its literal tuple type — that's what gives you typed `queryClient.getQueryData(myOptions.queryKey)`.

### `infiniteQueryOptions`

```ts
function infiniteQueryOptions<
  TQueryFnData,
  TError = Error,
  TData = InfiniteData<TQueryFnData>,
  TQueryKey extends readonly unknown[] = readonly unknown[],
  TPageParam = unknown
>(
  options: CreateInfiniteQueryOptions<TQueryFnData, TError, TData, TQueryKey, TPageParam>
): CreateInfiniteQueryOptions<TQueryFnData, TError, TData, TQueryKey, TPageParam>
```

### `mutationOptions`

```ts
function mutationOptions<
  TData = unknown,
  TError = Error,
  TVariables = void,
  TOnMutateResult = unknown
>(
  options: WithRequired<
    CreateMutationOptions<TData, TError, TVariables, TOnMutateResult>,
    'mutationKey'
  >
): WithRequired<CreateMutationOptions<TData, TError, TVariables, TOnMutateResult>, 'mutationKey'>

function mutationOptions<...>(
  options: Omit<CreateMutationOptions<TData, TError, TVariables, TOnMutateResult>, 'mutationKey'>
): Omit<CreateMutationOptions<TData, TError, TVariables, TOnMutateResult>, 'mutationKey'>
```

Two overloads: one forces `mutationKey` to be required (for filtering via `useIsMutating` / `useMutationState`), the other omits it entirely. The first is what `mutationOptions()` defaults to; if you don't want a key, call `createMutation` directly.

### `useQueryClient`

```ts
function useQueryClient(queryClient?: QueryClient): QueryClient
```

Returns the `QueryClient` from context. If you pass an explicit client, that's returned instead (useful for testing or when you have multiple clients).

### `useIsFetching`

```ts
function useIsFetching(
  filters?: QueryFilters<readonly unknown[]>,
  queryClient?: QueryClient
): ReactiveValue<number>
```

Reactive count of queries currently fetching, optionally narrowed by `filters` (`{ queryKey, type, status, ... }`).

### `useIsMutating`

```ts
function useIsMutating(
  filters?: MutationFilters<unknown, Error, unknown, unknown>,
  queryClient?: QueryClient
): ReactiveValue<number>
```

Reactive count of mutations currently in flight, optionally narrowed by `filters` (`{ mutationKey, status, ... }`).

### `useMutationState`

```ts
function useMutationState<TResult = MutationState<unknown, Error, unknown, unknown>>(
  options?: MutationStateOptions<TResult>,
  queryClient?: QueryClient
): TResult[]
```

Reactive array of mutation states, optionally filtered and selected. `MutationStateOptions<TResult>`:

```ts
type MutationStateOptions<TResult> = {
  filters?: MutationFilters
  select?: (mutation: Mutation<unknown, DefaultError, unknown, unknown>) => TResult
}
```

Default `TResult` is `MutationState` — has `status`, `data`, `error`, `variables`, `context`, etc.

### `useIsRestoring`

```ts
function useIsRestoring(): Box<boolean>
```

Returns a `Box<boolean>` — true while a `HydrationBoundary` is in the process of restoring a dehydrated state into the cache. Rarely needed directly; useful for gating work that should run only after hydration completes.

### `useHydrate`

```ts
function useHydrate(
  state?: unknown,
  options?: HydrateOptions,
  queryClient?: QueryClient
): void
```

Imperatively hydrate a dehydrated state into the cache. The `HydrationBoundary` component wraps this; reach for the function only if you're building custom hydration.

### `getQueryClientContext`

```ts
function getQueryClientContext(): QueryClient
```

Low-level: returns the `QueryClient` from Svelte's context. Throws if none has been set. Prefer `useQueryClient()` in app code.

### `setQueryClientContext`

```ts
function setQueryClientContext(client: QueryClient): void
```

Low-level: sets a `QueryClient` on Svelte's context. `QueryClientProvider` does this for you.

### `getIsRestoringContext`

```ts
function getIsRestoringContext(): Box<boolean>
```

Low-level: returns the `isRestoring` flag from context.

### `setIsRestoringContext`

```ts
function setIsRestoringContext(isRestoring: Box<boolean>): void
```

Low-level: sets the `isRestoring` flag on context. Used internally by `HydrationBoundary`.

---

## Components

### `QueryClientProvider`

```svelte
<QueryClientProvider client={queryClient}>
  <!-- children snippet -->
</QueryClientProvider>
```

Props type:

```ts
type QueryClientProviderProps = {
  client: QueryClient
  children: Snippet
}
```

Puts `client` on Svelte's context so descendants can call `useQueryClient()` (and the `create*` functions, which use it internally).

### `HydrationBoundary`

```svelte
<HydrationBoundary state={dehydratedState}>
  <!-- children -->
</HydrationBoundary>
```

Reads the dehydrated state (produced by `dehydrate(queryClient)` from `@tanstack/query-core`) and writes it into the nearest `QueryClient` on mount. Useful for SSR scenarios where you serialize the server-side cache and ship it to the client.

Note: in the published type declarations, `QueryClientProvider` is actually re-exported as an alias of `HydrationBoundary` — they're closely related components. The two are typically used together: `QueryClientProvider` to set up the client, `HydrationBoundary` to inject dehydrated state.

---

## Type aliases

Full list of exported type aliases — for reference, you usually don't need to import these directly because the `create*` functions infer them.

### Options types

- **`Accessor<T>`** = `() => T` — the thunk wrapper required by v6.
- **`CreateBaseQueryOptions<TQueryFnData, TError, TData, TQueryData, TQueryKey>`** = `QueryObserverOptions<...>` — the full set of options accepted by `createQuery` (and shared with `createInfiniteQuery` minus the pagination options).
- **`CreateQueryOptions<TQueryFnData, TError, TData, TQueryKey>`** = `CreateBaseQueryOptions<TQueryFnData, TError, TData, TQueryFnData, TQueryKey>` — the typical "I'm calling createQuery" options type.
- **`CreateInfiniteQueryOptions<TQueryFnData, TError, TData, TQueryKey, TPageParam>`** = `InfiniteQueryObserverOptions<...>` — adds `initialPageParam`, `getNextPageParam`, `getPreviousPageParam`, `maxPages`.
- **`CreateMutationOptions<TData, TError, TVariables, TOnMutateResult>`** = `OmitKeyof<MutationObserverOptions<...>, '_defaulted'>` — `mutationFn` + lifecycle callbacks + retry config.
- **`DefinedInitialDataOptions<TQueryFnData, TError, TData, TQueryKey>`** = `CreateQueryOptions<...>` & `{ initialData: NonUndefinedGuard<TQueryFnData> | (() => NonUndefinedGuard<TQueryFnData>) }` — for `queryOptions`/`createQuery` overloads where `initialData` is provided.
- **`UndefinedInitialDataOptions<TQueryFnData, TError, TData, TQueryKey>`** = `CreateQueryOptions<...>` & `{ initialData?: InitialDataFunction<NonUndefinedGuard<TQueryFnData>> }` — for the no-`initialData` overload.
- **`MutationStateOptions<TResult>`** = `{ filters?: MutationFilters; select?: (mutation) => TResult }` — for `useMutationState`.
- **`QueriesOptions<T>`** — Tuple type for `createQueries`'s `queries` array.
- **`QueriesResults<T>`** — Tuple type for the result array of `createQueries` (without `combine`).

### Result types

- **`CreateBaseQueryResult<TData, TError>`** = `QueryObserverResult<TData, TError>` — full result object (`status`, `data`, `error`, `isPending`, `isFetching`, `refetch`, etc.).
- **`CreateQueryResult<TData, TError>`** = `CreateBaseQueryResult<TData, TError>` — alias used by `createQuery`.
- **`DefinedCreateBaseQueryResult<TData, TError>`** — same as `CreateBaseQueryResult` but with `data: TData` (non-undefined). Used when `initialData` is provided.
- **`DefinedCreateQueryResult<TData, TError>`** — alias used by `createQuery`'s `DefinedInitialDataOptions` overload.
- **`CreateInfiniteQueryResult<TData, TError>`** = `InfiniteQueryObserverResult<TData, TError>` — adds `hasNextPage`, `hasPreviousPage`, `isFetchingNextPage`, `isFetchingPreviousPage`, `fetchNextPage`, `fetchPreviousPage`.
- **`CreateBaseMutationResult<TData, TError, TVariables, TOnMutateResult>`** = `MutationObserverResult<...>` & `{ mutateAsync: CreateMutateAsyncFunction<...> }` — full mutation result.
- **`CreateMutationResult<TData, TError, TVariables, TOnMutateResult>`** = `CreateBaseMutationResult<...>` — alias used by `createMutation`.

### Function types (mutation callbacks)

- **`CreateMutateFunction<TData, TError, TVariables, TOnMutateResult>`** = `(...args: Parameters<MutateFunction<...>>) => void` — the type of `mutation.mutate`.
- **`CreateMutateAsyncFunction<TData, TError, TVariables, TOnMutateResult>`** — the type of `mutation.mutateAsync`; returns `Promise<TData>`.

### Component types

- **`QueryClientProviderProps`** = `{ client: QueryClient; children: Snippet }` — props for `<QueryClientProvider>`.
- **`HydrationBoundary`** = `SvelteComponent` — the component type for `<HydrationBoundary>`.

## `@tanstack/query-core` re-exports

svelte-query re-exports a lot from `@tanstack/query-core` (its peer dependency). The most-used re-exports you'll likely reach for:

- `QueryClient` — the cache-holding class
- `QueryCache` / `MutationCache` — accessible via `queryClient.getQueryCache()` / `queryClient.getMutationCache()`
- `dehydrate` / `hydrate` / `HydrateOptions` — for SSR
- `keepPreviousData` — for `placeholderData: keepPreviousData`
- `CancelledError` / `isCancelledError` — for cancellation handling
- `QueryFunction`, `QueryKey`, `MutationKey`, `QueryFilters`, `MutationFilters` — utility types

If you import something and TypeScript says it's not exported from `@tanstack/svelte-query`, try `@tanstack/query-core` instead.
