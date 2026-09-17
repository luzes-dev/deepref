<script lang="ts">
	import SearchIcon from '@lucide/svelte/icons/search';
	import XIcon from '@lucide/svelte/icons/x';
	import type { Table } from '@tanstack/table-core';
	import type { ReportDto } from '$lib/api/generated/models';
	import { Button } from '@deepref/ui/button';
	import * as InputGroup from '@deepref/ui/input-group';
	import ArticleDataTableCitationsFilter from './ArticleDataTableCitationsFilter.svelte';
	import ArticleDataTableFacetedFilter from './ArticleDataTableFacetedFilter.svelte';
	import ArticleDataTableViewOptions from './ArticleDataTableViewOptions.svelte';
	import ArticleDataTableYearFilter from './ArticleDataTableYearFilter.svelte';

	type FilterOption = {
		label: string;
		value: string;
	};

	let { table, articles }: { table: Table<ReportDto>; articles: ReportDto[] } = $props();

	const isFiltered = $derived(table.getState().columnFilters.length > 0);
	const titleColumn = $derived(table.getColumn('title'));
	const typeColumn = $derived(table.getColumn('type'));
	const yearColumn = $derived(table.getColumn('issued_year'));
	const internalColumn = $derived(table.getColumn('internal_citations'));
	const maxInternal = $derived(
		Math.max(0, ...articles.map((article) => article.internal_citations))
	);
	const typeOptions = $derived.by<FilterOption[]>(() => {
		const labels = new Set(articles.map((article) => article.type ?? 'Unknown'));
		return Array.from(labels)
			.sort((a, b) => a.localeCompare(b))
			.map((value) => ({ label: value, value }));
	});
	const yearBounds = $derived.by(() => {
		const years = articles
			.map((article) => article.issued_year)
			.filter((year): year is number => typeof year === 'number');

		return years.length
			? {
					min: Math.min(...years),
					max: Math.max(...years)
				}
			: undefined;
	});
</script>

<div class="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
	<div class="flex flex-1 flex-col gap-2 lg:flex-row lg:items-center">
		<InputGroup.Root class="h-8 lg:max-w-72">
			<InputGroup.Input
				placeholder="Search title, DOI, or report ID"
				aria-label="Search articles"
				value={(titleColumn?.getFilterValue() as string) ?? ''}
				oninput={(event) => titleColumn?.setFilterValue(event.currentTarget.value)}
				onchange={(event) => titleColumn?.setFilterValue(event.currentTarget.value)}
			/>
			<InputGroup.Addon><SearchIcon /></InputGroup.Addon>
		</InputGroup.Root>

		<div class="flex flex-wrap items-center gap-2">
			{#if typeColumn && typeOptions.length > 0}
				<ArticleDataTableFacetedFilter
					column={typeColumn}
					title="Type"
					options={typeOptions}
				/>
			{/if}
			{#if yearColumn && yearBounds}
				<ArticleDataTableYearFilter
					column={yearColumn}
					minYear={yearBounds.min}
					maxYear={yearBounds.max}
				/>
			{/if}
			{#if internalColumn}
				<ArticleDataTableCitationsFilter
					column={internalColumn}
					maxCitations={maxInternal}
				/>
			{/if}
			{#if isFiltered}
				<Button
					variant="ghost"
					size="sm"
					class="h-8"
					onclick={() => table.resetColumnFilters()}
				>
					Reset
					<XIcon data-icon="inline-end" />
				</Button>
			{/if}
		</div>
	</div>
	<ArticleDataTableViewOptions {table} />
</div>
