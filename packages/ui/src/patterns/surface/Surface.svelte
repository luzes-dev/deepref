<script lang="ts">
	import type { Snippet } from 'svelte';
	import { cn } from '../../internal/utils';

	let {
		children,
		as = 'div',
		tone = 'default',
		class: className = '',
		label
	}: {
		children?: Snippet;
		as?: 'div' | 'section' | 'article' | 'aside';
		tone?: 'plain' | 'default' | 'subtle' | 'raised' | 'inset';
		class?: string;
		label?: string;
	} = $props();

	const toneClass = $derived(
		{
			plain: 'rounded-none bg-transparent',
			default: 'bg-card',
			subtle: 'bg-muted',
			raised: 'border border-border bg-popover shadow-overlay',
			inset: 'border border-border-subtle bg-surface-inset'
		}[tone]
	);
</script>

<svelte:element
	this={as}
	class={cn(
		'min-w-0 text-foreground',
		toneClass,
		tone === 'plain' ? undefined : 'rounded-lg',
		className
	)}
	aria-label={label}
	data-surface
	data-tone={tone}
>
	{@render children?.()}
</svelte:element>
