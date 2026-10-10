<script lang="ts">
	import './layout.css';
	import favicon from '#lib/assets/favicon.svg';
	import { Toaster } from '@deepref/ui/sonner';
	import { createAppQueryClient } from '#lib/api/query-client.js';
	import { PageFrame } from '@deepref/ui/layout';
	import { routeMetaForPathname } from '#lib/routes.js';
	import { page } from '$app/state';
	import { ModeWatcher } from 'mode-watcher';
	import { QueryClientProvider } from '@tanstack/svelte-query';
	import NotificationsWatcher from '#lib/features/notifications/NotificationsWatcher.svelte';
	import SettingsDialog from '#lib/features/settings/SettingsDialog.svelte';

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
	<SettingsDialog />
	<NotificationsWatcher />
	<!-- Top placement keeps confirmations off the sticky decision bars at the bottom. -->
	<Toaster position="top-center" offset={56} closeButton />
</QueryClientProvider>
