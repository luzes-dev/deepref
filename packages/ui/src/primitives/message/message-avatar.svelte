<script lang="ts">
	import type { HTMLAttributes } from 'svelte/elements';
	import { cn, type WithElementRef } from '../../internal/utils.js';

	let {
		ref = $bindable(null),
		class: className,
		anchor = 'top',
		children,
		...restProps
	}: WithElementRef<HTMLAttributes<HTMLDivElement>> & {
		anchor?: 'top' | 'bottom';
	} = $props();
</script>

<div
	bind:this={ref}
	data-slot="message-avatar"
	data-anchor={anchor}
	class={cn(
		'shrink-0 select-none',
		anchor === 'bottom' ? 'self-end' : 'self-start',
		className
	)}
	{...restProps}
>
	{@render children?.()}
</div>
