<script lang="ts">
	import type { HTMLAttributes } from 'svelte/elements';
	import { cn, type WithElementRef } from '../../internal/utils.js';

	let {
		ref = $bindable(null),
		class: className,
		align = 'start',
		children,
		...restProps
	}: WithElementRef<HTMLAttributes<HTMLDivElement>> & {
		align?: 'start' | 'end';
	} = $props();
</script>

<div
	bind:this={ref}
	data-slot="message-root"
	data-align={align}
	class={cn(
		'group/message flex w-full gap-3',
		align === 'end' ? 'flex-row-reverse justify-start' : 'flex-row justify-start',
		className
	)}
	{...restProps}
>
	{@render children?.()}
</div>
