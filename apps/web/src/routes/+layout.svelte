<script lang="ts">
	import './layout.css';
	import favicon from '$lib/assets/favicon.svg';
	import { Toaster } from '$lib/components/ui/sonner';
	import { createAppQueryClient } from '$lib/api/query-client';
	import { PageFrame } from '$lib/components/layout';
	import { routeMetaForPathname } from '$lib/routes';
	import { page } from '$app/state';
	import { ModeWatcher } from 'mode-watcher';
	import { QueryClientProvider } from '@tanstack/svelte-query';

	let { children } = $props();
	const queryClient = createAppQueryClient();
	const routeMeta = $derived(routeMetaForPathname(page.url.pathname));
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
	<title>{routeMeta.title}</title>
	<meta name="description" content={routeMeta.description} />
</svelte:head>

<ModeWatcher />
<QueryClientProvider client={queryClient}>
	<PageFrame>
		{@render children()}
	</PageFrame>
	<Toaster />
</QueryClientProvider>
