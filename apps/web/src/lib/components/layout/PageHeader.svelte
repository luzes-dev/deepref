<script lang="ts">
	import type { Snippet } from 'svelte';
	import { cn } from '$lib/utils';

	let {
		eyebrow,
		title,
		description,
		children,
		actions,
		class: className = ''
	}: {
		eyebrow?: string;
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
	<div class="min-w-0">
		{#if eyebrow}
			<p class="mb-1 text-xs font-semibold tracking-[0.14em] text-primary uppercase">
				{eyebrow}
			</p>
		{/if}
		<h1 class="editorial-title text-3xl leading-tight text-foreground sm:text-4xl">{title}</h1>
		{#if description}
			<p class="mt-2 max-w-2xl text-sm leading-relaxed text-muted-foreground">
				{description}
			</p>
		{/if}
		{#if children}{@render children()}{/if}
	</div>
	{#if actions}
		<div class="flex shrink-0 items-center gap-2">{@render actions()}</div>
	{/if}
</header>
