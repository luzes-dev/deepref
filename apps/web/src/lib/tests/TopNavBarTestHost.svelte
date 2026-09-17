<script lang="ts">
	import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query';
	import type { Snippet } from 'svelte';
	import TopNavBar from '$lib/shell/TopNavBar.svelte';

	let {
		children,
		...topNavBarProps
	}: {
		children?: Snippet;
		title?: string;
		titleSnippet?: Snippet;
		scopeLabel?: string;
		projects?: Array<{ id: string; name: string }>;
		selectedProjectId?: string | null;
		onSelectProject?: (projectId: string | null) => void;
		onCreateProject?: () => void;
		agentLabel?: string;
		agentHref?: string;
		onAgentClick?: () => void;
		agentSnippet?: Snippet;
		leftSnippet?: Snippet;
		rightSnippet?: Snippet;
		class?: string;
	} = $props();

	const queryClient = new QueryClient({
		defaultOptions: {
			queries: { retry: false, refetchOnWindowFocus: false },
			mutations: { retry: false }
		}
	});
</script>

<QueryClientProvider client={queryClient}>
	<TopNavBar {...topNavBarProps} />
	{@render children?.()}
</QueryClientProvider>
