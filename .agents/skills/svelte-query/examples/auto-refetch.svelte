<!-- examples/auto-refetch.svelte
  refetchInterval driven by reactive $state.
  Also shows a mutation that invalidates the query on success.
-->
<script lang="ts">
  import {
    useQueryClient,
    createQuery,
    createMutation,
  } from '@tanstack/svelte-query'

  let intervalMs = $state(1000)
  let value = $state('')

  const qc = useQueryClient()

  type Todos = { items: string[]; ts: number }

  const todos = createQuery<Todos>(() => ({
    queryKey: ['refetch'],
    queryFn: async () => await fetch('/api/data').then((r) => r.json()),
    // Polling interval — reactive: changing intervalMs updates polling rate
    refetchInterval: intervalMs,
  }))

  const addMutation = createMutation(() => ({
    mutationFn: (value: string) =>
      fetch(`/api/data?add=${encodeURIComponent(value)}`).then((r) => r.json()),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['refetch'] }),
  }))

  const onSubmit = (e: SubmitEvent) => {
    e.preventDefault()
    if (!value.trim()) return
    addMutation.mutate(value, {
      onSuccess: () => (value = ''),
    })
  }
</script>

<h1>Auto Refetch (every {intervalMs}ms)</h1>

<label>
  Polling interval (ms):
  <input type="number" bind:value={intervalMs} step="100" min="500" />
  <span
    style="display:inline-block; margin-left:.5rem; width:.75rem; height:.75rem;
           border-radius:50%; background: {todos.isFetching ? 'green' : 'transparent'};
           transition: {!todos.isFetching ? 'all .3s ease' : 'none'};"
  ></span>
</label>

<form onsubmit={onSubmit}>
  <input bind:value placeholder="Add an item" />
  <button disabled={addMutation.isPending || !value.trim()}>Add</button>
</form>

{#if todos.isPending}
  <p>Loading…</p>
{:else if todos.isError}
  <p>Error: {todos.error.message}</p>
{:else}
  <ul>
    {#each todos.data.items as item}
      <li>{item}</li>
    {/each}
  </ul>
  <small>Last updated: {new Date(todos.data.ts).toLocaleTimeString()}</small>
{/if}
