<!-- examples/basic-query.svelte
  Minimal createQuery example: fetch a list, branch on status.
  Drop into a SvelteKit route or Vite + Svelte 5 app that has a
  QueryClientProvider in scope.
-->
<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'

  type Post = { id: number; title: string; body: string }

  const fetchPosts = async (limit = 10): Promise<Post[]> => {
    const res = await fetch(
      `https://jsonplaceholder.typicode.com/posts?_limit=${limit}`,
    )
    return await res.json()
  }

  const posts = createQuery<Post[]>(() => ({
    queryKey: ['posts', 10],
    queryFn: () => fetchPosts(10),
  }))
</script>

<h1>Basic Query</h1>

{#if posts.isPending}
  <p>Loading…</p>
{:else if posts.isError}
  <p>Error: {posts.error.message}</p>
{:else if posts.isSuccess}
  <ul>
    {#each posts.data as post}
      <li>
        <strong>{post.id}.</strong> {post.title}
      </li>
    {/each}
  </ul>
  {#if posts.isFetching}
    <p><em>Background refreshing…</em></p>
  {/if}
{/if}
