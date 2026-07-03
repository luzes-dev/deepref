<script lang="ts" module>
	type TData = unknown;
</script>

<script lang="ts" generics="TData">
	import ChevronLeftIcon from '@lucide/svelte/icons/chevron-left';
	import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
	import ChevronsLeftIcon from '@lucide/svelte/icons/chevrons-left';
	import ChevronsRightIcon from '@lucide/svelte/icons/chevrons-right';
	import type { Table } from '@tanstack/table-core';
	import { Button } from '$lib/components/ui/button';
	import * as Select from '$lib/components/ui/select';

	let { table }: { table: Table<TData> } = $props();
	const pageCount = $derived(Math.max(1, table.getPageCount()));
</script>

<div class="flex flex-col gap-3 px-2 lg:flex-row lg:items-center lg:justify-between">
	<div class="text-sm text-muted-foreground">
		{table.getFilteredSelectedRowModel().rows.length} of
		{table.getFilteredRowModel().rows.length} row(s) selected.
	</div>
	<div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-end lg:gap-8">
		<div class="flex items-center gap-2">
			<p class="text-sm font-medium">Rows per page</p>
			<Select.Root
				allowDeselect={false}
				type="single"
				value={`${table.getState().pagination.pageSize}`}
				onValueChange={(value) => table.setPageSize(Number(value))}
			>
				<Select.Trigger class="h-8 w-[70px]">
					{String(table.getState().pagination.pageSize)}
				</Select.Trigger>
				<Select.Content side="top">
					<Select.Group>
						{#each [10, 20, 30, 40, 50] as pageSize (pageSize)}
							<Select.Item value={`${pageSize}`}>
								{pageSize}
							</Select.Item>
						{/each}
					</Select.Group>
				</Select.Content>
			</Select.Root>
		</div>
		<div class="flex items-center gap-2">
			<div class="flex w-24 items-center justify-center text-sm font-medium">
				Page {table.getState().pagination.pageIndex + 1} of {pageCount}
			</div>
			<div class="flex items-center gap-2">
				<Button
					variant="outline"
					size="icon-sm"
					class="hidden lg:flex"
					onclick={() => table.setPageIndex(0)}
					disabled={!table.getCanPreviousPage()}
				>
					<span class="sr-only">Go to first page</span>
					<ChevronsLeftIcon />
				</Button>
				<Button
					variant="outline"
					size="icon-sm"
					onclick={() => table.previousPage()}
					disabled={!table.getCanPreviousPage()}
				>
					<span class="sr-only">Go to previous page</span>
					<ChevronLeftIcon />
				</Button>
				<Button
					variant="outline"
					size="icon-sm"
					onclick={() => table.nextPage()}
					disabled={!table.getCanNextPage()}
				>
					<span class="sr-only">Go to next page</span>
					<ChevronRightIcon />
				</Button>
				<Button
					variant="outline"
					size="icon-sm"
					class="hidden lg:flex"
					onclick={() => table.setPageIndex(table.getPageCount() - 1)}
					disabled={!table.getCanNextPage()}
				>
					<span class="sr-only">Go to last page</span>
					<ChevronsRightIcon />
				</Button>
			</div>
		</div>
	</div>
</div>
