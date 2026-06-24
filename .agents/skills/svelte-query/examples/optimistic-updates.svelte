<!-- examples/optimistic-updates.svelte
  Full optimistic-update pattern: snapshot, write optimistically,
  roll back on error, refetch on settle.
-->
<script lang="ts">
  import {
    useQueryClient,
    createQuery,
    createMutation,
  } from '@tanstack/svelte-query'

  type Todo = { id: string; text: string }
  type Todos = { items: readonly Todo[]; ts: number }

  const qc = useQueryClient()

  let text = $state('')

  const todos = createQuery<Todos>(() => ({
    queryKey: ['todos'],
    queryFn: async () => await fetch('/api/todos').then((r) => r.json()),
  }))

  const addTodo = createMutation(() => ({
    mutationFn: (text: string) =>
      fetch('/api/todos', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ text }),
      }).then((r) => r.json()),

    onMutate: async (newText: string) => {
      // 1. Cancel any in-flight refetches so they don't clobber our write
      await qc.cancelQueries({ queryKey: ['todos'] })

      // 2. Snapshot for rollback
      const previousTodos = qc.getQueryData<Todos>(['todos'])

      // 3. Optimistically update the cache
      if (previousTodos) {
        qc.setQueryData<Todos>(['todos'], {
          ...previousTodos,
          items: [
            ...previousTodos.items,
            { id: crypto.randomUUID(), text: newText },
          ],
        })
      }

      // 4. Return context — becomes the 3rd arg to onError/onSuccess
      return { previousTodos }
    },

    onError: (_err, _vars, context) => {
      // Roll back to the snapshot if the mutation failed
      if (context?.previousTodos) {
        qc.setQueryData(['todos'], context.previousTodos)
      }
    },

    onSettled: () => {
      // Always refetch to converge with server truth
      qc.invalidateQueries({ queryKey: ['todos'] })
    },
  }))

  const onSubmit = (e: SubmitEvent) => {
    e.preventDefault()
    if (!text.trim()) return
    addTodo.mutate(text)
    text = ''
  }
</script>

<h1>Optimistic Updates</h1>

<p>
  New items appear instantly. If the server rejects the mutation,
  the previous list is restored.
</p>

<form onsubmit={onSubmit}>
  <input bind:value={text} placeholder="New todo…" />
  <button disabled={addTodo.isPending || !text.trim()}>Add</button>
</form>

{#if todos.isPending}
  <p>Loading…</p>
{:else if todos.isError}
  <p>Error: {todos.error.message}</p>
{:else}
  <ul>
    {#each todos.data.items as todo}
      <li>{todo.text}</li>
    {/each}
  </ul>
  <small>Updated: {new Date(todos.data.ts).toLocaleTimeString()}</small>
{/if}

{#if todos.isFetching}
  <p style="color: green">Background refreshing…</p>
{/if}
