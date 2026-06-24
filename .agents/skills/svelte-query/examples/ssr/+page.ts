// examples/ssr/+page.ts
// Prefetch the posts query on the server (and on client-side
// navigations). The cached data is then found by createQuery
// in +page.svelte, so the first render has data with no flash.
import type { PageLoad } from './$types'

export const load: PageLoad = async ({ parent, fetch }) => {
  const { queryClient } = await parent()

  // IMPORTANT: use SvelteKit's `fetch` (the second arg of load),
  // not global fetch. It handles relative URLs server-side and
  // tracks deps for invalidation.
  await queryClient.prefetchQuery({
    queryKey: ['posts', 10],
    queryFn: async () => {
      const res = await fetch('https://jsonplaceholder.typicode.com/posts?_limit=10')
      return await res.json()
    },
  })
}
