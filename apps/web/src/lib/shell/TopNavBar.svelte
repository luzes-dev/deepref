<script lang="ts">
	import type { Snippet } from 'svelte';
	import { cn } from '#lib/utils.js';
	import DependencyHealthIndicator from './DependencyHealthIndicator.svelte';
	import NotificationBell from './NotificationBell.svelte';

	let {
		title = 'Overview',
		titleSnippet,
		rightSnippet,
		class: className = '',
		projectId = null
	}: {
		title?: string;
		titleSnippet?: Snippet;
		rightSnippet?: Snippet;
		class?: string;
		/** The project in view; the bell and the health badge are scoped to it. */
		projectId?: string | null;
	} = $props();
</script>

<header
	class={cn(
		'relative flex h-12 w-full items-center justify-between gap-3 border-b border-border/50 bg-background px-4 text-foreground sm:px-6',
		className
	)}
	data-top-navbar
>
	<div class="min-w-0">
		{#if titleSnippet}
			{@render titleSnippet()}
		{:else}
			<h1 class="truncate text-sm font-medium tracking-tight text-foreground">
				{title}
			</h1>
		{/if}
	</div>

	<div class="flex shrink-0 items-center gap-2">
		<DependencyHealthIndicator {projectId} />
		<NotificationBell {projectId} />
		{#if rightSnippet}
			{@render rightSnippet()}
		{/if}
	</div>
</header>
