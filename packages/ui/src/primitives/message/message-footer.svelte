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
	data-slot="message-footer"
	class={cn(
		'flex items-center gap-2 text-xs text-muted-foreground',
		'group-data-[align=end]/message:justify-end',
		className
	)}
	{...restProps}
>
	{@render children?.()}
</div>
