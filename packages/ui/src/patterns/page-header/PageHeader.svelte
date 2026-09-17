<script lang="ts">
	import type { Snippet } from 'svelte';
	import { cn } from '../../internal/utils';

	let {
		title,
		description,
		children,
		actions,
		class: className = ''
	}: {
		title: string;
		description?: string;
		children?: Snippet;
		actions?: Snippet;
		class?: string;
	} = $props();
</script>

<header
	class={cn('flex flex-wrap items-start justify-between gap-section', className)}
	data-page-header
>
	<div class="min-w-0 flex-1 basis-64">
		<h1 class="editorial-title text-xl leading-tight break-words text-foreground sm:text-2xl">{title}</h1>
		{#if description}
			<p class="mt-2 max-w-2xl text-sm leading-relaxed text-muted-foreground">
				{description}
			</p>
		{/if}
		{#if children}{@render children()}{/if}
	</div>
	{#if actions}
		<div class="flex max-w-full flex-wrap items-center gap-2">{@render actions()}</div>
	{/if}
</header>
