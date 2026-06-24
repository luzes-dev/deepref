<script lang="ts">
	import './layout.css';
	import favicon from '$lib/assets/favicon.svg';
	import AppHeader from '$lib/components/app/AppHeader.svelte';
	import { Toaster } from '$lib/components/ui/sonner';
	import { createAppQueryClient } from '$lib/api/query-client';
	import { ModeWatcher } from 'mode-watcher';
	import { QueryClientProvider } from '@tanstack/svelte-query';

	let { children } = $props();
	const queryClient = createAppQueryClient();
</script>

<svelte:head><link rel="icon" href={favicon} /></svelte:head>

<ModeWatcher />
<QueryClientProvider client={queryClient}>
	<div class="min-h-svh bg-background text-foreground">
		<AppHeader />
		<main class="mx-auto max-w-7xl px-4 py-6">
			{@render children()}
		</main>
	</div>
	<Toaster />
</QueryClientProvider>
