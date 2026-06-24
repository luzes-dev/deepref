// examples/ssr/+layout.ts
// Creates a per-request QueryClient. With `enabled: browser`,
// queries don't auto-run on the server (avoiding unresolvable
// promises after HTML is sent). prefetchQuery() in +page.ts
// still works because it's an explicit call.
import { browser } from '$app/environment'
import { QueryClient } from '@tanstack/svelte-query'
import type { LayoutLoad } from './$types'

export const load: LayoutLoad = () => {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: {
        // Critical: stop queries from auto-fetching on the server
        enabled: browser,
        // Recommended: don't refetch immediately on hydration
        staleTime: 60 * 1000,
      },
    },
  })

  return { queryClient }
}
