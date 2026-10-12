<script lang="ts">
	import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query';
	import FullTextScreening from '#lib/features/full-text/components/FullTextScreening.svelte';

	let { projectId }: { projectId: string } = $props();
	const client = new QueryClient({
		defaultOptions: {
			queries: { retry: false, refetchOnWindowFocus: false },
			mutations: { retry: false }
		}
	});
</script>

<QueryClientProvider {client}>
	<FullTextScreening {projectId} />
</QueryClientProvider>
