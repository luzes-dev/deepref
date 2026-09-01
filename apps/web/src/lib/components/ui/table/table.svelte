<script lang="ts">
	import { onMount } from 'svelte';
	import type { HTMLTableAttributes } from 'svelte/elements';
	import { cn, type WithElementRef } from '$lib/utils.js';

	type TableProps = WithElementRef<HTMLTableAttributes> & {
		containerLabel: string;
	};

	let {
		ref = $bindable(null),
		class: className,
		containerLabel,
		children,
		...restProps
	}: TableProps = $props();

	let container: HTMLDivElement;
	let scrollable = $state(false);

	onMount(() => {
		const updateScrollable = () => {
			scrollable = container.scrollWidth > container.clientWidth + 1;
		};
		const observer = new ResizeObserver(updateScrollable);
		observer.observe(container);
		if (ref) observer.observe(ref);
		updateScrollable();
		return () => observer.disconnect();
	});
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div
	bind:this={container}
	data-slot="table-container"
	class="relative w-full overflow-x-auto"
	tabindex={scrollable ? 0 : undefined}
	role="region"
	aria-label={containerLabel}
>
	<table
		bind:this={ref}
		data-slot="table"
		class={cn('w-full caption-bottom text-sm', className)}
		{...restProps}
	>
		{@render children?.()}
	</table>
</div>
