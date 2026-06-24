# Mutations reference

In-depth reference for `createMutation`, `mutationOptions`, the mutation lifecycle, optimistic updates, and invalidation strategies. Read this when you need to handle mutation side-effects, roll back on errors, or coordinate cache updates after writes.

## Table of contents
- createMutation
- mutationOptions helper
- mutate vs mutateAsync
- Lifecycle callbacks
- Per-call callbacks
- Optimistic updates (with rollback)
- Invalidation strategies
- useMutationState
- Patterns

## createMutation

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

`Accessor<T>` is `() => T` — the runes thunk. `TData` is what `mutationFn` resolves to. `TVariables` is what callers pass to `mutate(...)`. `TContext` is the value returned from `onMutate` and passed to `onError`/`onSuccess`/`onSettled` (typically used to carry a snapshot for rollback).

```svelte
<script lang="ts">
  import { createMutation, useQueryClient } from '@tanstack/svelte-query'

  const qc = useQueryClient()

  const addTodo = createMutation(() => ({
    mutationFn: (text: string) =>
      fetch('/api/todos', { method: 'POST', body: JSON.stringify({ text }) })
        .then((r) => r.json()),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['todos'] }),
  }))
</script>

<button onclick={() => addTodo.mutate('hello')} disabled={addTodo.isPending}>
  Add
</button>

{#if addTodo.isPending}Saving…{/if}
{#if addTodo.isError}Error: {addTodo.error.message}{/if}
{#if addTodo.isSuccess}Saved!{/if}
```

## mutationOptions helper

`mutationOptions` is the mutation equivalent of `queryOptions` — a factory that locks in types and is shareable across components.

```ts
import { mutationOptions } from '@tanstack/svelte-query'

export const addTodoOptions = () =>
  mutationOptions({
    mutationKey: ['add-todo'],
    mutationFn: (text: string) =>
      fetch('/api/todos', { method: 'POST', body: JSON.stringify({ text }) })
        .then((r) => r.json()),
  })
```

Note: `mutationOptions` requires `mutationKey` (the helper forces it). The key lets you filter with `useIsMutating` and `useMutationState`. If you don't need filtering, you can call `createMutation` directly without going through `mutationOptions`.

Two overloads: one with required `mutationKey`, one without.

## mutate vs mutateAsync

`mutate(variables, callbacks?)` — fire and forget. Returns `void`. Errors go to the `onError` callback (per-call or component-level).

```ts
addTodo.mutate('hello', {
  onSuccess: (data) => console.log('saved', data),
  onError: (err) => console.error('failed', err),
  onSettled: () => console.log('done'),
})
```

`mutateAsync(variables)` — returns a `Promise<TData>`. Rejects on error (so wrap in try/catch or use `.catch`). Use when you need to wait for the result before doing something else.

```ts
try {
  const saved = await addTodo.mutateAsync('hello')
  await navigate(`/todos/${saved.id}`)
} catch (err) {
  show toast
}
```

Per-call callbacks don't combine with `mutateAsync` the same way — you can still pass them, but you'll also get the promise result/rejection. Pick one mechanism per call site.

## Lifecycle callbacks

These are defined on `createMutation`'s options and run for every mutation invocation in this order:

1. **`onMutate(variables) => TContext | Promise<TContext>`** — Runs before `mutationFn`. Use to snapshot state for rollback or perform optimistic updates. Whatever you return becomes the `context` argument to the next callbacks.
2. **`mutationFn(variables) => Promise<TData>`** — The actual side effect.
3. **`onError(error, variables, context) => void`** — Runs if `mutationFn` rejected. Use `context` (the value you returned from `onMutate`) to roll back optimistic updates.
4. **`onSuccess(data, variables, context) => void`** — Runs if `mutationFn` resolved. Use to invalidate queries, write to the cache directly, show a success toast, etc.
5. **`onSettled(data | undefined, error | null, variables, context) => void`** — Always runs, after success or error. Good for hiding spinners or refetching regardless of outcome.

If `onMutate` throws, the chain skips to `onError` with that thrown value as the error.

Per-call callbacks (the second argument to `mutate(...)`) fire **after** the component-level ones, in the same order. They won't fire if the component unmounts before the mutation settles — for that reason, prefer component-level callbacks for state cleanup.

## Optimistic updates (with rollback)

The canonical "show the change immediately, revert if the server rejects it" pattern:

```svelte
<script lang="ts">
  import {
    useQueryClient,
    createQuery,
    createMutation,
  } from '@tanstack/svelte-query'

  type Todo = { id: string; text: string }
  type Todos = { items: readonly Todo[]; ts: number }

  const qc = useQueryClient()

  const todos = createQuery<Todos>(() => ({
    queryKey: ['todos'],
    queryFn: fetchTodos,
  }))

  const addTodo = createMutation(() => ({
    mutationFn: (text: string) =>
      fetch('/api/todos', { method: 'POST', body: JSON.stringify({ text }) })
        .then((r) => r.json()),

    onMutate: async (newText: string) => {
      // Cancel any in-flight refetches so they don't clobber our update
      await qc.cancelQueries({ queryKey: ['todos'] })

      // Snapshot for rollback
      const previousTodos = qc.getQueryData<Todos>(['todos'])

      // Optimistically write the new state
      if (previousTodos) {
        qc.setQueryData<Todos>(['todos'], {
          ...previousTodos,
          items: [
            ...previousTodos.items,
            { id: crypto.randomUUID(), text: newText },
          ],
        })
      }

      // Return context — this becomes the 3rd arg to onError/onSuccess
      return { previousTodos }
    },

    onError: (_err, _vars, context) => {
      // Roll back to the snapshot
      if (context?.previousTodos) {
        qc.setQueryData(['todos'], context.previousTodos)
      }
    },

    onSettled: () => {
      // Always refetch to converge with server truth
      qc.invalidateQueries({ queryKey: ['todos'] })
    },
  }))
</script>

<button onclick={() => addTodo.mutate('hello')} disabled={addTodo.isPending}>
  Add
</button>
```

The four-step dance — cancel, snapshot, write, rollback — is the standard. Skipping the cancel can cause a race where a background refetch overwrites your optimistic update just before the mutation settles.

## Invalidation strategies

After a mutation succeeds, you typically want related queries to refetch. Three approaches, in order of preference:

### 1. Invalidate by prefix

```ts
onSuccess: () => qc.invalidateQueries({ queryKey: ['todos'] })
```

Invalidates every query whose key starts with `['todos', ...]` — the list, detail queries, anything. Marks them stale; if they're being observed by a mounted component, they refetch.

### 2. Invalidate everything

```ts
onSuccess: () => qc.invalidateQueries()
```

Use sparingly — only when a mutation truly affects every part of the cache (rare).

### 3. Write directly to the cache

If you know exactly what the new entity looks like (the mutation returns it), skip the refetch:

```ts
onSuccess: (updatedTodo) => {
  // Update the list
  qc.setQueryData<Todo[]>(['todos'], (old) =>
    old?.map((t) => (t.id === updatedTodo.id ? updatedTodo : t))
  )
  // Update the detail
  qc.setQueryData(['todo', updatedTodo.id], updatedTodo)
}
```

This is faster (no extra request) but riskier (you must keep the cache shape consistent). Pair with `invalidateQueries` if you're not 100% sure.

### Force refetch even when fresh

```ts
qc.invalidateQueries({
  queryKey: ['todos'],
  refetchType: 'active',  // default, but explicit
})
qc.invalidateQueries({
  queryKey: ['todos'],
  refetchType: 'none',    // mark stale but don't refetch
})
qc.invalidateQueries({
  queryKey: ['todos'],
  refetchType: 'all',     // refetch even inactive queries
})
```

### `removeQueries` — drop from cache entirely

```ts
qc.removeQueries({ queryKey: ['todo', deletedId] })
```

Used after a delete mutation, when you don't want any stale data lingering.

## useMutationState

Subscribe to the mutation cache. Useful for showing a global list of recent mutations, or inspecting the state of mutations not owned by this component.

```ts
import { useMutationState } from '@tanstack/svelte-query'

const pendingMutations = useMutationState({
  filters: { mutationKey: ['add-todo'], status: 'pending' },
  select: (mutation) => mutation.state.variables,
})
// pendingMutations is reactive: string[] of texts currently being added
```

Options:
- `filters?: MutationFilters` — narrow by `mutationKey`, `status`, `exact`, etc.
- `select?: (mutation) => TResult` — pick what to extract from each mutation.

Returns a reactive array.

## useIsMutating

Reactive count of in-flight mutations matching a filter. Useful for global "saving…" indicators.

```ts
import { useIsMutating } from '@tanstack/svelte-query'

const savingCount = useIsMutating({ mutationKey: ['add-todo'] })
// savingCount.value === 2 when two add-todo mutations are in flight
```

Note: `useIsMutating` returns a `ReactiveValue<number>`, so read it as `savingCount.value` in script and `savingCount` (auto-unwrap) in templates.

## Patterns

### Conditional callbacks per call

```ts
addTodo.mutate(text, {
  onSuccess: () => {
    // Only this call's success path
    navigate('/todos')
  },
})
```

### Sequential mutations

```ts
const createThenPublish = createMutation(() => ({
  mutationFn: async (text: string) => {
    const created = await createTodo(text)
    await publishTodo(created.id)
    return created
  },
  onSuccess: () => qc.invalidateQueries({ queryKey: ['todos'] }),
}))
```

Or chain with `mutateAsync`:

```ts
const created = await createTodo.mutateAsync(text)
await publishTodo.mutateAsync(created.id)
```

### Mutation with progress (no built-in; use `onMutate` + `onSettled`)

```svelte
<script lang="ts">
  let saving = $state(false)

  const upload = createMutation(() => ({
    mutationFn: (file: File) => uploadFile(file),
    onMutate: () => { saving = true },
    onSettled: () => { saving = false },
  }))
</script>

<input
  type="file"
  onchange={(e) => upload.mutate(e.currentTarget.files![0])}
/>
{#if saving}Uploading…{/if}
```

### Form submit with optimistic update + rollback on validation error

```ts
const saveForm = createMutation(() => ({
  mutationFn: (values) => api.patch('/profile', values),
  onMutate: async (values) => {
    await qc.cancelQueries({ queryKey: ['profile'] })
    const previous = qc.getQueryData(['profile'])
    qc.setQueryData(['profile'], (old) => ({ ...old, ...values }))
    return { previous }
  },
  onError: (_e, _v, ctx) => {
    if (ctx?.previous) qc.setQueryData(['profile'], ctx.previous)
  },
  onSettled: () => qc.invalidateQueries({ queryKey: ['profile'] }),
}))
```

## Result fields reference

The object returned by `createMutation` is reactive.

- **`mutate(variables, callbacks?)`** — fire and forget.
- **`mutateAsync(variables)`** — returns a `Promise<TData>`.
- **`status: 'idle' | 'pending' | 'success' | 'error'`** — Discriminated status. Starts `'idle'`.
- **`isIdle` / `isPending` / `isSuccess` / `isError`** — Boolean flags.
- **`data: TData | undefined`** — Result of the last successful mutation. `undefined` until first success.
- **`error: TError | null`** — Error from the last failed mutation. `null` until first error.
- **`variables: TVariables | undefined`** — The variables passed to the most recent `mutate(...)`. Useful for displaying what was being submitted.
- **`context: TContext | undefined`** — The value returned from `onMutate` for the current/last mutation.
- **`submittedAt: number`** — Timestamp (ms) of the last successful mutation.
- **`reset()`** — Resets the mutation back to `idle` state, clearing `data`, `error`, etc.
- **`failureCount` / `failureReason`** — Retry tracking (mutations don't retry by default, but you can opt in).

## Mutation options reference

- **`mutationKey: MutationKey`** — Optional (required by `mutationOptions` helper). For filtering in `useIsMutating` / `useMutationState`.
- **`mutationFn: (variables) => Promise<TData>`** — Required. The side effect.
- **`onMutate` / `onError` / `onSuccess` / `onSettled`** — Lifecycle callbacks described above.
- **`retry: number | boolean | ((count, error) => boolean)`** — Default `0`. Mutations don't retry unless you opt in.
- **`retryDelay: (attempt) => number`** — Backoff function.
- **`gcTime: number`** — How long the mutation stays in cache after settle (default `5 * 60 * 1000`).
- **`networkMode: 'online' | 'always' | 'offlineFirst'`** — Default `'online'`. With `'always'`, mutations recorded while offline will retry when online.
- **`scope: { id: string }`** — Mutations sharing a `scope.id` run serially (one at a time). Useful for ordered writes.
- **`meta: Record<string, unknown>`** — User-defined metadata; available in the context passed to `mutationFn` and callbacks.
