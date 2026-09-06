<script lang="ts">
	import type { Snippet } from 'svelte';
	import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query';

	let {
		client = new QueryClient({
			defaultOptions: {
				queries: { retry: false, gcTime: Infinity },
				mutations: { retry: false }
			}
		}),
		children
	}: {
		client?: QueryClient;
		children: Snippet;
	} = $props();
</script>

<QueryClientProvider {client}>
	{@render children()}
</QueryClientProvider>
