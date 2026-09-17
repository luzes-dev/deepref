<script lang="ts">
	import { cn, type WithElementRef } from '../../internal/utils.js';
	import type { HTMLAttributes } from 'svelte/elements';

	let {
		ref = $bindable(null),
		class: className,
		children,
		...restProps
	}: WithElementRef<HTMLAttributes<HTMLTableRowElement>> = $props();
</script>

<tr
	bind:this={ref}
	data-slot="table-row"
	class={cn(
		'border-b transition-colors hover:bg-interactive-hover data-[state=selected]:bg-selection',
		className
	)}
	{...restProps}
>
	{@render children?.()}
</tr>
