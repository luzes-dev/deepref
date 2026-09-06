<script lang="ts">
	import type { Snippet } from 'svelte';
	import { cn } from '$lib/utils';

	let {
		children,
		navigation,
		title,
		description,
		class: className = '',
		mainClass = '',
		mainId = 'main-content'
	}: {
		children: Snippet;
		navigation?: Snippet;
		title?: string;
		description?: string;
		class?: string;
		mainClass?: string;
		mainId?: string;
	} = $props();
</script>

<div class={cn('min-h-svh bg-background text-foreground', className)} data-page-frame>
	<a class="skip-link" href={`#${mainId}`}>Skip to content</a>
	{#if navigation}
		{@render navigation()}
	{/if}
	<main id={mainId} class={cn('min-h-svh min-w-0', mainClass)} tabindex="-1">
		{#if title}
			<div class="sr-only">
				<h1>{title}</h1>
				{#if description}<p>{description}</p>{/if}
			</div>
		{/if}
		{@render children()}
	</main>
</div>
