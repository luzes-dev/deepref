<script lang="ts">
	import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query';
	import type { Snippet } from 'svelte';
	import NotificationBell from '$lib/shell/NotificationBell.svelte';

	let { children, client }: { children?: Snippet; client?: QueryClient } = $props();

	const queryClient = $derived(
		client ??
			new QueryClient({
				defaultOptions: {
					queries: { retry: false, refetchOnWindowFocus: false },
					mutations: { retry: false }
				}
			})
	);
</script>

<QueryClientProvider client={queryClient}>
	<NotificationBell />
	{@render children?.()}
</QueryClientProvider>
