<script lang="ts">
	import type { ScreeningQueueItemDto } from '$lib/api/generated/models';
	import { createVirtualizer } from '@tanstack/svelte-virtual';
	import { getCoreRowModel, type ColumnDef, type RowData } from '@tanstack/table-core';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import { createSvelteTable } from '$lib/components/ui/data-table';
	import { get } from 'svelte/store';

	type Row = ScreeningQueueItemDto & RowData;

	let {
		items,
		selectedReport,
		loading,
		hasNextPage,
		loadingNextPage,
		onSelect,
		onLoadMore
	}: {
		items: ScreeningQueueItemDto[];
		selectedReport: string | null;
		loading: boolean;
		hasNextPage: boolean;
		loadingNextPage: boolean;
		onSelect: (reportId: string) => void;
		onLoadMore: () => void | Promise<void>;
	} = $props();

	let scrollElement = $state<HTMLDivElement | null>(null);
	const columns: ColumnDef<Row>[] = [
		{ id: 'title', header: 'Title', accessorKey: 'title' },
		{ id: 'year', header: 'Year', accessorKey: 'publication_year' },
		{ id: 'status', header: 'Status', accessorKey: 'title_abstract_status' }
	];
	const table = createSvelteTable({
		get data() {
			return items as Row[];
		},
		columns,
		getRowId: (row) => row.report_id,
		getCoreRowModel: getCoreRowModel()
	});
	const virtualizer = createVirtualizer<HTMLDivElement, HTMLDivElement>({
		count: 0,
		getScrollElement: () => scrollElement,
		estimateSize: () => 72,
		overscan: 8
	});
	let virtualCount = 0;

	$effect(() => {
		const count = items.length;
		if (count === virtualCount) return;
		virtualCount = count;
		get(virtualizer).setOptions({ count });
	});

	function handleScroll() {
		const last = $virtualizer.getVirtualItems().at(-1);
		if (last && last.index >= items.length - 5 && hasNextPage && !loadingNextPage) {
			void onLoadMore();
		}
	}
</script>

<Card.Root class="min-h-[28rem]">
	<Card.Header class="flex-row items-center justify-between gap-4">
		<div>
			<Card.Title>Table mode</Card.Title>
			<Card.Description>Server-paginated and virtualized reports.</Card.Description>
		</div>
		{#if loadingNextPage}<span class="text-xs text-muted-foreground">Loading more…</span>{/if}
	</Card.Header>
	<Card.Content class="p-0">
		<div
			bind:this={scrollElement}
			onscroll={handleScroll}
			class="h-[32rem] overflow-auto border-y"
			role="table"
			aria-label="Title and abstract screening queue"
		>
			<div
				class="sticky top-0 z-10 grid grid-cols-[minmax(0,1fr)_6rem_8rem] border-b bg-background px-4 py-3 text-xs font-medium text-muted-foreground"
			>
				{#each table.getHeaderGroups()[0]?.headers ?? [] as header (header.id)}
					<div role="columnheader">{header.column.columnDef.header as string}</div>
				{/each}
			</div>
			{#if loading && items.length === 0}
				<p class="p-6 text-sm text-muted-foreground">Loading reports…</p>
			{:else if items.length === 0}
				<p class="p-6 text-sm text-muted-foreground">No reports match these filters.</p>
			{:else}
				<div style={`height: ${$virtualizer.getTotalSize()}px; position: relative;`}>
					{#each $virtualizer.getVirtualItems() as virtualRow (virtualRow.key)}
						{@const row = table.getRowModel().rows[virtualRow.index]}
						{#if row}
							<button
								type="button"
								role="row"
								class="absolute left-0 grid w-full grid-cols-[minmax(0,1fr)_6rem_8rem] gap-0 border-b px-4 py-3 text-left text-sm hover:bg-muted/50 data-[selected=true]:bg-muted"
								data-selected={row.original.report_id === selectedReport
									? 'true'
									: undefined}
								style={`transform: translateY(${virtualRow.start}px);`}
								onclick={() => onSelect(row.original.report_id)}
							>
								<span role="cell" class="truncate pr-3 font-medium"
									>{row.original.title ?? 'Untitled report'}</span
								>
								<span role="cell">{row.original.publication_year ?? '—'}</span>
								<span role="cell" class="capitalize"
									>{row.original.title_abstract_status}</span
								>
							</button>
						{/if}
					{/each}
				</div>
			{/if}
		</div>
		{#if hasNextPage}
			<div class="flex justify-center p-4">
				<Button
					variant="outline"
					disabled={loadingNextPage}
					onclick={() => void onLoadMore()}
				>
					{loadingNextPage ? 'Loading…' : 'Load more reports'}
				</Button>
			</div>
		{/if}
	</Card.Content>
</Card.Root>
