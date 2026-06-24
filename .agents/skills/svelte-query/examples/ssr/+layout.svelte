<!-- examples/ssr/+layout.svelte
  Wraps the app in QueryClientProvider. The QueryClient comes
  from +layout.ts (one per browser session, fresh per server request).
-->
<script lang="ts">
  import '../app.css'
  import { browser } from '$app/environment'
  import { QueryClientProvider } from '@tanstack/svelte-query'
  import { SvelteQueryDevtools } from '@tanstack/svelte-query-devtools'
  import type { LayoutData } from './$types'

  const { data, children } = $props<{ data: LayoutData }>()
</script>

<QueryClientProvider client={data.queryClient}>
  <main>
    {@render children()}
  </main>

  {#if browser}
    <SvelteQueryDevtools />
  {/if}
</QueryClientProvider>
