<script lang="ts">
	import type { Snippet } from 'svelte';
	import { cn } from '../../internal/utils';

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
		'flex flex-wrap items-center justify-between gap-control border-b border-subtle py-control',
		className
	)}
	role="group"
	data-page-toolbar
	aria-label={label}
>
	<div class="flex min-w-0 flex-wrap items-center gap-2">
		{#if leading}{@render leading()}{/if}
		{#if children}{@render children()}{/if}
	</div>
	{#if trailing}<div class="flex max-w-full flex-wrap items-center gap-2">{@render trailing()}</div>{/if}
</div>
