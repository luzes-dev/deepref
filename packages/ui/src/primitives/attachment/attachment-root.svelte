<script lang="ts">
	import type { HTMLAnchorAttributes } from 'svelte/elements';
	import { cn, type WithElementRef } from '../../internal/utils.js';

	let {
		ref = $bindable(null),
		href,
		class: className,
		children,
		...restProps
	}: WithElementRef<HTMLAnchorAttributes> & {
		href?: string;
	} = $props();
</script>

<svelte:element
	this={href ? 'a' : 'div'}
	bind:this={ref}
	data-slot="attachment-root"
	{href}
	class={cn(
		'group/attachment inline-flex max-w-xs items-center gap-1.5 rounded-md border border-border bg-card px-2 py-1 text-xs font-medium text-card-foreground shadow-2xs transition-colors',
		href &&
			'cursor-pointer hover:bg-muted hover:text-foreground focus-visible:border-ring focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring',
		className
	)}
	{...restProps}
>
	{@render children?.()}
</svelte:element>
