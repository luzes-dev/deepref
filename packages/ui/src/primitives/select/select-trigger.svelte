<script lang="ts">
	import { Select as SelectPrimitive } from 'bits-ui';
	import { cn, type WithoutChild } from '../../internal/utils.js';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';

	let {
		ref = $bindable(null),
		class: className,
		children,
		size = 'default',
		...restProps
	}: WithoutChild<SelectPrimitive.TriggerProps> & {
		size?: 'sm' | 'default';
	} = $props();
</script>

<SelectPrimitive.Trigger
	bind:ref
	data-slot="select-trigger"
	data-size={size}
	class={cn(
		"flex min-w-0 max-w-full w-fit items-center justify-between gap-1.5 rounded-md border border-input bg-card text-foreground py-2 pr-2 pl-2.5 text-sm whitespace-nowrap transition-[color,box-shadow] outline-none focus-visible:border-ring focus-visible:ring-0 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 data-placeholder:text-muted-foreground data-[size=default]:h-control-height data-[size=sm]:h-8 *:data-[slot=select-value]:line-clamp-1 *:data-[slot=select-value]:flex *:data-[slot=select-value]:flex *:data-[slot=select-value]:items-center *:data-[slot=select-value]:gap-1.5 hover:bg-interactive-hover dark:aria-invalid:ring-destructive/40 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
		className
	)}
	{...restProps}
>
	<span class="min-w-0 flex-1 truncate text-left">{@render children?.()}</span>
	<ChevronDownIcon class="pointer-events-none size-4 text-muted-foreground" />
</SelectPrimitive.Trigger>
