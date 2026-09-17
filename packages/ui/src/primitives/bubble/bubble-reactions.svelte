<script lang="ts">
	import type { HTMLAttributes } from 'svelte/elements';
	import { cn, type WithElementRef } from '../../internal/utils.js';

	let {
		ref = $bindable(null),
		class: className,
		children,
		...restProps
	}: WithElementRef<HTMLAttributes<HTMLDivElement>> = $props();
</script>

<div
	bind:this={ref}
	data-slot="bubble-reactions"
	class={cn(
		'flex items-center gap-1 mt-1 opacity-80 transition-opacity hover:opacity-100',
		className
	)}
	{...restProps}
>
	{@render children?.()}
</div>
