<script lang="ts">
	import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query';
	import type { Snippet } from 'svelte';
	import NotificationBell from '$lib/shell/NotificationBell.svelte';

	let {
		children,
		client,
		projectId = null
	}: { children?: Snippet; client?: QueryClient; projectId?: string | null } = $props();

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
	<NotificationBell {projectId} />
	{@render children?.()}
</QueryClientProvider>
