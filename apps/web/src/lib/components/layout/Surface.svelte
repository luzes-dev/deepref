<script lang="ts">
	import type { Snippet } from 'svelte';
	import { cn } from '$lib/utils';

	let {
		children,
		as = 'div',
		tone = 'default',
		class: className = '',
		label
	}: {
		children?: Snippet;
		as?: 'div' | 'section' | 'article' | 'aside';
		tone?: 'default' | 'subtle' | 'raised' | 'inset';
		class?: string;
		label?: string;
	} = $props();

	const toneClass = $derived(
		{
			default: 'bg-card ring-1 ring-foreground/10',
			subtle: 'bg-muted/45 ring-1 ring-border/65',
			raised: 'bg-card shadow-sm ring-1 ring-foreground/10',
			inset: 'bg-background ring-1 ring-border/70'
		}[tone]
	);
</script>

<svelte:element
	this={as}
	class={cn('min-w-0 rounded-lg text-card-foreground', toneClass, className)}
	aria-label={label}
	data-surface
	data-tone={tone}
>
	{@render children?.()}
</svelte:element>
