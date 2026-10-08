<script lang="ts" generics="TData">
	import type { Table } from '@tanstack/table-core';
	import { Button } from '@deepref/ui/button';
	import * as Select from '@deepref/ui/select';
	import ChevronLeftIcon from '@lucide/svelte/icons/chevron-left';
	import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
	let { table }: { table: Table<TData> } = $props();
</script>

<div class="flex flex-wrap items-center justify-between gap-3 text-xs text-muted-foreground">
	<span class="tabular-nums">{table.getFilteredRowModel().rows.length} matching articles</span>
	<div class="flex items-center gap-3">
		<Select.Root
			type="single"
			value={String(table.getState().pagination.pageSize)}
			onValueChange={(value) => {
				if (value) table.setPageSize(Number(value));
			}}
		>
			<Select.Trigger aria-label="Articles per page" class="w-28"
				>{table.getState().pagination.pageSize} per page</Select.Trigger
			>
			<Select.Content
				><Select.Group
					>{#each [10, 25, 50, 100] as size (size)}<Select.Item
							value={String(size)}
							label={`${size} per page`}
						/>{/each}</Select.Group
				></Select.Content
			>
		</Select.Root>
		<span class="tabular-nums"
			>{table.getState().pagination.pageIndex + 1} / {Math.max(1, table.getPageCount())}</span
		>
		<Button
			variant="ghost"
			size="icon-sm"
			aria-label="Previous page"
			disabled={!table.getCanPreviousPage()}
			onclick={() => table.previousPage()}><ChevronLeftIcon /></Button
		>
		<Button
			variant="ghost"
			size="icon-sm"
			aria-label="Next page"
			disabled={!table.getCanNextPage()}
			onclick={() => table.nextPage()}><ChevronRightIcon /></Button
		>
	</div>
</div>
