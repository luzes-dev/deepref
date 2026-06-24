<!-- examples/ssr/+page.svelte
  Reads the query that was prefetched in +page.ts. The cache
  is already populated, so query.data is defined on first render
  (no loading flash on initial page load).
-->
<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'

  type Post = { id: number; title: string; body: string }

  const query = createQuery<Post[]>(() => ({
    queryKey: ['posts', 10],
    queryFn: async () => {
      const res = await fetch('https://jsonplaceholder.typicode.com/posts?_limit=10')
      return await res.json()
    },
  }))
</script>

<h1>Posts (SSR-prefetched)</h1>

{#if query.isPending}
  <p>Loading…</p>
{:else if query.isError}
  <p>Error: {query.error.message}</p>
{:else}
  <ul>
    {#each query.data as post}
      <li>
        <strong>{post.id}.</strong> {post.title}
      </li>
    {/each}
  </ul>
  {#if query.isFetching}
    <p><em>Background refreshing…</em></p>
  {/if}
{/if}
