<script lang="ts">
	import * as ResizablePrimitive from 'paneforge';
	import { cn, type WithoutChildrenOrChild } from '../../internal/utils.js';

	let {
		ref = $bindable(null),
		class: className,
		withHandle = false,
		...restProps
	}: WithoutChildrenOrChild<ResizablePrimitive.PaneResizerProps> & {
		withHandle?: boolean;
	} = $props();
</script>

<ResizablePrimitive.PaneResizer
	bind:ref
	data-slot="resizable-handle"
	class={cn(
		'cn-resizable-handle group relative flex w-px items-center justify-center bg-border transition-colors hover:bg-border-strong data-active:bg-ring!',
		'after:absolute after:inset-y-0 after:left-1/2 after:w-2.5 after:-translate-x-1/2',
		'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring',
		'data-[direction=vertical]:h-px data-[direction=vertical]:w-full data-[direction=vertical]:after:left-0 data-[direction=vertical]:after:h-2.5 data-[direction=vertical]:after:w-full data-[direction=vertical]:after:translate-x-0 data-[direction=vertical]:after:-translate-y-1/2 [&[data-direction=vertical]>div]:rotate-90',
		className
	)}
	{...restProps}
>
	{#if withHandle}
		<div
			class="z-10 flex h-7 w-4 shrink-0 items-center justify-center rounded-md bg-border-strong text-background shadow-xs transition-colors group-hover:bg-text-secondary group-data-active:bg-ring!"
		>
			<svg width="8" height="12" viewBox="0 0 8 12" fill="currentColor" aria-hidden="true">
				<circle cx="2" cy="1.5" r="1.2" />
				<circle cx="6" cy="1.5" r="1.2" />
				<circle cx="2" cy="6" r="1.2" />
				<circle cx="6" cy="6" r="1.2" />
				<circle cx="2" cy="10.5" r="1.2" />
				<circle cx="6" cy="10.5" r="1.2" />
			</svg>
		</div>
	{/if}
</ResizablePrimitive.PaneResizer>
