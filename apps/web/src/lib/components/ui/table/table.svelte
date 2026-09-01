<script lang="ts">
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
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div
	data-slot="table-container"
	class="relative w-full overflow-x-auto"
	tabindex="0"
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
