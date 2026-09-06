<script lang="ts">
	import type { ScreeningQueueItemDto } from '$lib/api/generated/models';
	import { createVirtualizer } from '@tanstack/svelte-virtual';
	import { getCoreRowModel, type ColumnDef, type RowData } from '@tanstack/table-core';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import { createSvelteTable } from '$lib/components/ui/data-table';
	import { Badge } from '$lib/components/ui/badge';
	import { get } from 'svelte/store';
	import { FileText, LoaderCircle } from '@lucide/svelte';

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

	function statusLabel(status: string) {
		return status.replaceAll('_', ' ');
	}
</script>

<Card.Root class="min-h-[28rem]" data-testid="screening-table">
	<Card.Header class="gap-3 border-b border-border/60 pb-4">
		<div class="flex flex-wrap items-start justify-between gap-3">
			<div class="flex items-center gap-2">
				<span
					class="flex size-8 items-center justify-center rounded-lg bg-primary/10 text-primary"
				>
					<FileText aria-hidden="true" />
				</span>
				<div>
					<Card.Title>Table mode</Card.Title>
					<Card.Description
						>Scan the queue and select a report to open focus mode.</Card.Description
					>
				</div>
			</div>
			{#if loadingNextPage}
				<span
					class="flex items-center gap-1.5 text-xs text-muted-foreground"
					aria-live="polite"
				>
					<LoaderCircle class="animate-spin" aria-hidden="true" /> Loading more…
				</span>
			{/if}
		</div>
	</Card.Header>
	<Card.Content class="p-0">
		<div
			bind:this={scrollElement}
			onscroll={handleScroll}
			class="h-[32rem] overflow-auto"
			role="table"
			aria-label="Title and abstract screening queue"
		>
			<div
				class="sticky top-0 z-10 grid grid-cols-[minmax(12rem,1fr)_5rem_7rem] border-b bg-card px-4 py-3 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase"
			>
				{#each table.getHeaderGroups()[0]?.headers ?? [] as header (header.id)}
					<div role="columnheader">{header.column.columnDef.header as string}</div>
				{/each}
			</div>
			{#if loading && items.length === 0}
				<div
					class="flex flex-col items-center justify-center gap-2 p-12 text-center text-sm text-muted-foreground"
				>
					<LoaderCircle class="animate-spin" aria-hidden="true" />
					<p>Loading reports…</p>
				</div>
			{:else if items.length === 0}
				<div class="flex flex-col items-center justify-center gap-2 p-12 text-center">
					<FileText class="text-muted-foreground" aria-hidden="true" />
					<p class="text-sm font-medium">No reports match these filters.</p>
					<p class="max-w-sm text-xs text-muted-foreground">
						Try another status, search term, or sort order.
					</p>
				</div>
			{:else}
				<div style={`height: ${$virtualizer.getTotalSize()}px; position: relative;`}>
					{#each $virtualizer.getVirtualItems() as virtualRow (virtualRow.key)}
						{@const row = table.getRowModel().rows[virtualRow.index]}
						{#if row}
							<button
								type="button"
								role="row"
								class="absolute left-0 grid min-h-[4.5rem] w-full grid-cols-[minmax(12rem,1fr)_5rem_7rem] gap-0 border-b border-border/60 px-4 py-3 text-left text-sm transition-colors hover:bg-muted/50 data-[selected=true]:bg-primary/10 data-[selected=true]:shadow-[inset_3px_0_0_var(--primary)]"
								data-selected={row.original.report_id === selectedReport
									? 'true'
									: undefined}
								style={`transform: translateY(${virtualRow.start}px);`}
								onclick={() => onSelect(row.original.report_id)}
							>
								<span role="cell" class="flex min-w-0 items-start gap-2 pr-3">
									<span
										class="mt-0.5 flex size-6 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground"
									>
										<FileText aria-hidden="true" />
									</span>
									<span class="min-w-0">
										<span class="block truncate font-medium"
											>{row.original.title ?? 'Untitled report'}</span
										>
										<span
											class="mt-1 block truncate text-xs text-muted-foreground"
											>{row.original.doi ?? 'Identifier unavailable'}</span
										>
									</span>
								</span>
								<span role="cell" class="pt-1 text-muted-foreground"
									>{row.original.publication_year ?? '—'}</span
								>
								<span role="cell" class="pt-0.5">
									<Badge
										variant={row.original.title_abstract_status === 'exclude'
											? 'destructive'
											: row.original.title_abstract_status === 'include'
												? 'default'
												: 'secondary'}
									>
										{statusLabel(row.original.title_abstract_status)}
									</Badge>
								</span>
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
