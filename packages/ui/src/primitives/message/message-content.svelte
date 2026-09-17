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
	data-slot="message-content"
	class={cn(
		'flex min-w-0 max-w-[85%] flex-col gap-1.5',
		'group-data-[align=end]/message:items-end group-data-[align=start]/message:items-start',
		className
	)}
	{...restProps}
>
	{@render children?.()}
</div>
