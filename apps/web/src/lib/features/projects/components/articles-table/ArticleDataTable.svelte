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
		type SortingState,
		type Updater,
		type VisibilityState
	} from '@tanstack/table-core';
	import type { ReportDto } from '#lib/api/generated/models/index.js';
	import { createSvelteTable, FlexRender } from '@deepref/ui/data-table';
	import * as Table from '@deepref/ui/table';
	import { cn } from '#lib/utils.js';
	import ArticleDataTablePagination from './ArticleDataTablePagination.svelte';
	import ArticleDataTableToolbar from './ArticleDataTableToolbar.svelte';
	import { createArticleColumns } from './columns.js';
	import { useProjectWorkspaceContext } from '../../context.svelte.js';

	type ArticleDataTableProps = {
		articles: ReportDto[];
		selectedArticle?: string;
		openArticle: (reportId: string) => void;
	};

	let { articles, selectedArticle, openArticle }: ArticleDataTableProps = $props();
	const workspace = useProjectWorkspaceContext();
	const routeFilter = $derived(workspace.articleFilters.filter);
	const routeMinInternal = $derived(workspace.articleFilters.minInternal);

	let columnVisibility = $state<VisibilityState>({
		outbound_internal_references: false,
		type: false,
		rank_score: false
	});
	let localColumnFilters = $state<ColumnFiltersState>([]);
	const columnFilters = $derived.by<ColumnFiltersState>(() => [
		...localColumnFilters,
		...(routeFilter ? [{ id: 'title', value: routeFilter }] : []),
		...(routeMinInternal > 0 ? [{ id: 'internal_citations', value: routeMinInternal }] : [])
	]);
	let sorting = $state<SortingState>([{ id: 'rank_score', desc: true }]);
	let pagination = $state<PaginationState>({ pageIndex: 0, pageSize: 25 });

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
		state: {
			get sorting() {
				return sorting;
			},
			get columnVisibility() {
				return columnVisibility;
			},
			get columnFilters() {
				return columnFilters;
			},
			get pagination() {
				return pagination;
			}
		},
		onSortingChange: (updater) => {
			sorting = updateState(updater, sorting);
		},
		onColumnFiltersChange: (updater) => {
			const next = updateState(updater, columnFilters);
			localColumnFilters = next.filter(
				({ id }) => id !== 'title' && id !== 'internal_citations'
			);
			const title = next.find(({ id }) => id === 'title')?.value;
			const internal = next.find(({ id }) => id === 'internal_citations')?.value;
			workspace.articleFilters.update(
				typeof title === 'string' ? title : '',
				typeof internal === 'number' ? internal : 0
			);
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

<div class="flex min-h-0 flex-1 flex-col gap-3">
	<ArticleDataTableToolbar {table} {articles} />
	<div
		class="min-h-0 flex-1 [&_[data-slot=table-container]]:h-full [&_[data-slot=table-container]]:overflow-auto"
	>
		<Table.Root containerLabel="Project articles">
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
						data-current={selectedArticle === row.original.report_id
							? 'true'
							: undefined}
						aria-current={selectedArticle === row.original.report_id
							? 'true'
							: undefined}
						class={cn(
							'data-[current=true]:bg-accent data-[current=true]:shadow-inset-accent'
						)}
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
	{#if table.getPageCount() > 1}<ArticleDataTablePagination {table} />{/if}
</div>
