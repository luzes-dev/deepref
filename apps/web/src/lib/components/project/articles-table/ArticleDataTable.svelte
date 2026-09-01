<script lang="ts">
	import {
		getCoreRowModel,
		getFacetedRowModel,
		getFacetedUniqueValues,
		getFilteredRowModel,
		getPaginationRowModel,
		getSortedRowModel,
		type ColumnFiltersState,
		type PaginationState,
		type RowSelectionState,
		type SortingState,
		type Updater,
		type VisibilityState
	} from '@tanstack/table-core';
	import type { ReportDto } from '$lib/api/generated/models';
	import { createSvelteTable, FlexRender } from '$lib/components/ui/data-table';
	import * as Table from '$lib/components/ui/table';
	import { cn } from '$lib/utils.js';
	import ArticleDataTablePagination from './ArticleDataTablePagination.svelte';
	import ArticleDataTableToolbar from './ArticleDataTableToolbar.svelte';
	import { createArticleColumns } from './columns.js';

	type ArticleDataTableProps = {
		articles: ReportDto[];
		selectedArticle?: string;
		openArticle: (reportId: string) => void;
	};

	let { articles, selectedArticle, openArticle }: ArticleDataTableProps = $props();

	let rowSelection = $state<RowSelectionState>({});
	let columnVisibility = $state<VisibilityState>({
		outbound_internal_references: false
	});
	let columnFilters = $state<ColumnFiltersState>([]);
	let sorting = $state<SortingState>([{ id: 'rank_score', desc: true }]);
	let pagination = $state<PaginationState>({ pageIndex: 0, pageSize: 10 });

	const columns = $derived(createArticleColumns({ openArticle, selectedArticle }));

	function updateState<T>(updater: Updater<T>, current: T) {
		return typeof updater === 'function' ? (updater as (value: T) => T)(current) : updater;
	}

	const table = createSvelteTable({
		get data() {
			return articles;
		},
		get columns() {
			return columns;
		},
		getRowId: (article) => article.report_id,
		enableRowSelection: true,
		state: {
			get sorting() {
				return sorting;
			},
			get columnVisibility() {
				return columnVisibility;
			},
			get rowSelection() {
				return rowSelection;
			},
			get columnFilters() {
				return columnFilters;
			},
			get pagination() {
				return pagination;
			}
		},
		onRowSelectionChange: (updater) => {
			rowSelection = updateState(updater, rowSelection);
		},
		onSortingChange: (updater) => {
			sorting = updateState(updater, sorting);
		},
		onColumnFiltersChange: (updater) => {
			columnFilters = updateState(updater, columnFilters);
		},
		onColumnVisibilityChange: (updater) => {
			columnVisibility = updateState(updater, columnVisibility);
		},
		onPaginationChange: (updater) => {
			pagination = updateState(updater, pagination);
		},
		getCoreRowModel: getCoreRowModel(),
		getFilteredRowModel: getFilteredRowModel(),
		getPaginationRowModel: getPaginationRowModel(),
		getSortedRowModel: getSortedRowModel(),
		getFacetedRowModel: getFacetedRowModel(),
		getFacetedUniqueValues: getFacetedUniqueValues()
	});
</script>

<div class="flex min-h-0 flex-1 flex-col gap-4">
	<ArticleDataTableToolbar {table} {articles} />
	<div
		class="min-h-0 flex-1 rounded-md border [&_[data-slot=table-container]]:h-full [&_[data-slot=table-container]]:overflow-auto"
	>
		<Table.Root>
			<Table.Header>
				{#each table.getHeaderGroups() as headerGroup (headerGroup.id)}
					<Table.Row>
						{#each headerGroup.headers as header (header.id)}
							<Table.Head
								colspan={header.colSpan}
								class="sticky top-0 z-10 bg-background"
							>
								{#if !header.isPlaceholder}
									<FlexRender
										content={header.column.columnDef.header}
										context={header.getContext()}
									/>
								{/if}
							</Table.Head>
						{/each}
					</Table.Row>
				{/each}
			</Table.Header>
			<Table.Body>
				{#each table.getRowModel().rows as row (row.id)}
					<Table.Row
						data-state={row.getIsSelected() && 'selected'}
						data-current={selectedArticle === row.original.report_id
							? 'true'
							: undefined}
						aria-current={selectedArticle === row.original.report_id
							? 'true'
							: undefined}
						class={cn('data-[current=true]:bg-muted/40')}
					>
						{#each row.getVisibleCells() as cell (cell.id)}
							<Table.Cell>
								<FlexRender
									content={cell.column.columnDef.cell}
									context={cell.getContext()}
								/>
							</Table.Cell>
						{/each}
					</Table.Row>
				{:else}
					<Table.Row>
						<Table.Cell
							colspan={columns.length}
							class="h-28 text-center text-muted-foreground"
						>
							No articles match the current filters.
						</Table.Cell>
					</Table.Row>
				{/each}
			</Table.Body>
		</Table.Root>
	</div>
	<ArticleDataTablePagination {table} />
</div>
