// examples/queries.ts
// Shared queryOptions factories. Define each query's key + fn + options
// in ONE place, then both +page.ts (prefetch) and +page.svelte (subscribe)
// reference the same factory. This guarantees key consistency.
import { queryOptions, infiniteQueryOptions } from '@tanstack/svelte-query'

export type Post = { id: number; title: string; body: string }

// --- Plain query ---
export const postsQuery = (limit: number = 10) =>
  queryOptions<Post[]>({
    queryKey: ['posts', limit],
    queryFn: async () => {
      const res = await fetch(
        `https://jsonplaceholder.typicode.com/posts?_limit=${limit}`,
      )
      return await res.json()
    },
    staleTime: 60_000,
  })

export const postQuery = (id: number) =>
  queryOptions<Post>({
    queryKey: ['post', id],
    queryFn: async () => {
      const res = await fetch(`https://jsonplaceholder.typicode.com/posts/${id}`)
      return await res.json()
    },
  })

// --- Infinite query ---
export const planetsQuery = infiniteQueryOptions({
  queryKey: ['planets'],
  queryFn: async ({ pageParam }) => {
    const res = await fetch(`https://swapi.dev/api/planets/?page=${pageParam}`)
    return await res.json()
  },
  initialPageParam: 1,
  getNextPageParam: (lastPage: { next: string | null }) => {
    if (!lastPage.next) return undefined
    const url = new URL(lastPage.next)
    return Number(url.searchParams.get('page'))
  },
})

// Usage in a component:
//   import { postsQuery } from '$lib/queries'
//   const query = createQuery(() => postsQuery(10))
//
// Usage in +page.ts:
//   import { postsQuery } from '$lib/queries'
//   export const load = async ({ parent }) => {
//     const { queryClient } = await parent()
//     await queryClient.prefetchQuery(postsQuery(10))
//   }
//
// Cache reads:
//   import { postsQuery } from '$lib/queries'
//   const cached = qc.getQueryData(postsQuery(10).queryKey)  // typed: Post[] | undefined
