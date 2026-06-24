<!-- examples/infinite-scroll.svelte
  createInfiniteQuery with a "Load more" button.
-->
<script lang="ts">
  import { createInfiniteQuery } from '@tanstack/svelte-query'

  type Planet = { name: string; population: string; url: string }
  type PlanetsPage = {
    count: number
    next: string | null
    previous: string | null
    results: Planet[]
  }

  const fetchPlanets = async (pageParam: number): Promise<PlanetsPage> => {
    const res = await fetch(`https://swapi.dev/api/planets/?page=${pageParam}`)
    return await res.json()
  }

  const query = createInfiniteQuery(() => ({
    queryKey: ['planets'],
    queryFn: ({ pageParam }) => fetchPlanets(pageParam),
    initialPageParam: 1,
    getNextPageParam: (lastPage) => {
      if (!lastPage.next) return undefined
      const url = new URL(lastPage.next)
      return Number(url.searchParams.get('page'))
    },
  }))
</script>

<h1>Planets — Infinite Scroll</h1>

{#if query.isPending}
  <p>Loading…</p>
{:else if query.isError}
  <p>Error: {query.error.message}</p>
{:else}
  {#each query.data.pages as page}
    {#each page.results as planet}
      <article>
        <h3>{planet.name}</h3>
        <p>Population: {planet.population}</p>
      </article>
    {/each}
  {/each}

  <button
    onclick={() => query.fetchNextPage()}
    disabled={!query.hasNextPage || query.isFetchingNextPage}
  >
    {#if query.isFetchingNextPage}
      Loading more…
    {:else if query.hasNextPage}
      Load more
    {:else}
      Nothing more to load
    {/if}
  </button>

  {#if query.isFetching && !query.isFetchingNextPage}
    <p><em>Background refreshing…</em></p>
  {/if}
{/if}

<style>
  article {
    margin-bottom: 1rem;
    padding: 1rem;
    border: 1px solid #ddd;
    border-radius: 4px;
  }
</style>
