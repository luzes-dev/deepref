<script lang="ts">
	import type { Snippet } from 'svelte';
	import { cn } from '$lib/utils';

	let {
		children,
		leading,
		trailing,
		label = 'Page controls',
		class: className = ''
	}: {
		children?: Snippet;
		leading?: Snippet;
		trailing?: Snippet;
		label?: string;
		class?: string;
	} = $props();
</script>

<div
	class={cn(
		'flex flex-wrap items-center justify-between gap-control border-y border-border/70 py-control',
		className
	)}
	data-page-toolbar
	aria-label={label}
>
	<div class="flex min-w-0 flex-wrap items-center gap-2">
		{#if leading}{@render leading()}{/if}
		{#if children}{@render children()}{/if}
	</div>
	{#if trailing}<div class="flex shrink-0 items-center gap-2">{@render trailing()}</div>{/if}
</div>
