<script lang="ts">
	import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query';
	import ScreeningFocus from '#lib/features/screening/components/ScreeningFocus.svelte';

	let { projectId }: { projectId: string } = $props();
	const client = new QueryClient({
		defaultOptions: {
			queries: { retry: false, refetchOnWindowFocus: false },
			mutations: { retry: false }
		}
	});
</script>

<QueryClientProvider {client}>
	<ScreeningFocus {projectId} />
</QueryClientProvider>
