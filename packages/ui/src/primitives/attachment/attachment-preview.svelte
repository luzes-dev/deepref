<script lang="ts">
	import type { HTMLAttributes } from 'svelte/elements';
	import { cn, type WithElementRef } from '../../internal/utils.js';

	let {
		ref = $bindable(null),
		class: className,
		children,
		...restProps
	}: WithElementRef<HTMLAttributes<HTMLSpanElement>> = $props();
</script>

<span
	bind:this={ref}
	data-slot="attachment-preview"
	class={cn(
		'flex shrink-0 items-center justify-center text-muted-foreground [&>svg]:size-3.5',
		className
	)}
	{...restProps}
>
	{@render children?.()}
</span>
