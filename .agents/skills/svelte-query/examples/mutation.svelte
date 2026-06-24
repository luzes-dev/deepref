<!-- examples/mutation.svelte
  A createMutation that adds a todo, then invalidates the list query.
-->
<script lang="ts">
  import {
    createQuery,
    createMutation,
    useQueryClient,
  } from '@tanstack/svelte-query'

  type Todo = { id: number; text: string; done: boolean }
  type Todos = { items: Todo[]; ts: number }

  const qc = useQueryClient()

  let text = $state('')

  const todos = createQuery<Todos>(() => ({
    queryKey: ['todos'],
    queryFn: async () =>
      await fetch('/api/todos').then((r) => r.json()),
  }))

  const addTodo = createMutation(() => ({
    mutationFn: (text: string) =>
      fetch('/api/todos', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ text }),
      }).then((r) => r.json()),
    onSuccess: () => {
      // Tell the cache the todos list is stale → refetch
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

<h1>Add Todo</h1>

<form onsubmit={onSubmit}>
  <input bind:value={text} placeholder="What needs to be done?" />
  <button disabled={addTodo.isPending || !text.trim()}>
    {addTodo.isPending ? 'Adding…' : 'Add'}
  </button>
</form>

{#if addTodo.isError}
  <p style="color: red">Failed: {addTodo.error.message}</p>
{/if}

{#if todos.isPending}
  <p>Loading todos…</p>
{:else if todos.isError}
  <p>Error: {todos.error.message}</p>
{:else}
  <ul>
    {#each todos.data.items as todo}
      <li>{todo.text}</li>
    {/each}
  </ul>
  <small>Last updated: {new Date(todos.data.ts).toLocaleTimeString()}</small>
{/if}
