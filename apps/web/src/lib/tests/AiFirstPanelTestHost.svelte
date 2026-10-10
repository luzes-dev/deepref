<script lang="ts">
	import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query';
	import AiFirstPanel from '$lib/features/ai-autonomy/components/AiFirstPanel.svelte';

	let { projectId }: { projectId: string } = $props();
	const client = new QueryClient({
		defaultOptions: {
			queries: { retry: false, refetchOnWindowFocus: false },
			mutations: { retry: false }
		}
	});
</script>

<QueryClientProvider {client}>
	<AiFirstPanel {projectId} />
</QueryClientProvider>
