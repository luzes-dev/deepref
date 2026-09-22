<script lang="ts">
	import * as Alert from '@deepref/ui/alert';
	import * as Empty from '@deepref/ui/empty';
	import * as InputGroup from '@deepref/ui/input-group';
	import * as Select from '@deepref/ui/select';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import { Skeleton } from '@deepref/ui/skeleton';
	import { Slider } from '@deepref/ui/slider';
	import PaginationLoadMore from '@deepref/ui/pagination-load-more';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import SearchIcon from '@lucide/svelte/icons/search';
	import PageTemplate from '$lib/shell/PageTemplate.svelte';
	import ArticleDataTable from './articles-table/ArticleDataTable.svelte';
	import { useProjectWorkspaceContext, type ArticleSort } from '../context.svelte.js';
	import { reportLabel, reportSearchText } from '../report-label';

	const workspace = useProjectWorkspaceContext();

	const sortLabels: Record<ArticleSort, string> = {
		rank: 'Rank score',
		internal: 'Internal citations',
		total: 'Total citations',
		year: 'Year',
		title: 'Title'
	};

	const filtered = $derived(
		workspace.articles
			.filter((article) => {
				const term = workspace.articleFilters.filter.toLowerCase();
				return (
					article.internal_citations >= workspace.articleFilters.minInternal &&
					reportSearchText(article).includes(term)
				);
			})
			.toSorted((a, b) => {
				if (workspace.articleFilters.sort === 'internal') {
					return b.internal_citations - a.internal_citations;
				}
				if (workspace.articleFilters.sort === 'total') {
					return b.total_citations - a.total_citations;
				}
				if (workspace.articleFilters.sort === 'year') {
					return (b.issued_year ?? 0) - (a.issued_year ?? 0);
				}
				if (workspace.articleFilters.sort === 'title') {
					return reportLabel(a).localeCompare(reportLabel(b));
				}
				return b.rank_score - a.rank_score;
			})
	);
	const staleMetrics = $derived(workspace.articles.filter((article) => article.metrics_stale));
	const latestMetricsAsOf = $derived.by(() => {
		const values = workspace.articles
			.map((article) => article.metrics_as_of)
			.filter((value): value is string => Boolean(value))
			.map(Date.parse)
			.filter(Number.isFinite);
		return values.length > 0 ? new Date(Math.max(...values)).toLocaleString() : undefined;
	});
</script>

<PageTemplate testId="articles-page" maxWidth="default">
	<div class="flex items-center justify-between gap-3 text-xs text-muted-foreground">
		<span>{workspace.articles.length.toLocaleString()} project articles</span>
		{#if latestMetricsAsOf}
			<span>Metrics as of {latestMetricsAsOf}</span>
		{/if}
	</div>

	<div class="grid gap-3 md:hidden" data-testid="article-mobile-filters">
		<InputGroup.Root>
			<InputGroup.Input
				placeholder="Search articles by title or DOI"
				aria-label="Search articles"
				bind:value={workspace.articleFilters.filter}
			/>
			<InputGroup.Addon><SearchIcon /></InputGroup.Addon>
		</InputGroup.Root>
		<Select.Root type="single" bind:value={workspace.articleFilters.sort}>
			<Select.Trigger class="w-full"
				>{sortLabels[workspace.articleFilters.sort]}</Select.Trigger
			>
			<Select.Content>
				<Select.Group>
					{#each Object.entries(sortLabels) as [value, label] (value)}
						<Select.Item {value} {label} />
					{/each}
				</Select.Group>
			</Select.Content>
		</Select.Root>
		<div class="flex items-center gap-3">
			<Slider
				type="single"
				bind:value={workspace.articleFilters.minInternal}
				max={20}
				step={1}
				thumbLabel="Minimum internal citations"
			/>
			<Badge variant="outline">Min {workspace.articleFilters.minInternal}</Badge>
		</div>
	</div>

	{#if staleMetrics.length > 0}
		<Alert.Root data-testid="stale-metrics-banner">
			<CircleAlertIcon />
			<Alert.Title>Metrics may be stale</Alert.Title>
			<Alert.Description>
				{staleMetrics.length} loaded article metrics are awaiting graph projection. Metrics as
				of {latestMetricsAsOf ?? 'not yet computed'}.
			</Alert.Description>
		</Alert.Root>
	{/if}

	{#if workspace.articlesError}
		<Alert.Root variant="destructive">
			<CircleAlertIcon />
			<Alert.Title>Articles unavailable</Alert.Title>
			<Alert.Description>{workspace.articlesError}</Alert.Description>
		</Alert.Root>
	{:else if workspace.articlesLoading}
		<div class="flex flex-col gap-2 p-4" aria-label="Loading articles">
			{#each [0, 1, 2, 3, 4, 5, 6, 7] as index (index)}
				<Skeleton class="h-12" />
			{/each}
		</div>
	{:else if workspace.articles.length === 0}
		<Empty.Root class="min-h-80 border-dashed">
			<Empty.Header>
				<Empty.Title>No articles</Empty.Title>
				<Empty.Description>Start an ingestion to populate this project.</Empty.Description>
			</Empty.Header>
		</Empty.Root>
	{:else}
		<div class="hidden min-h-0 flex-1 flex-col gap-3 md:flex">
			{#key workspace.selectedProjectId}
				<ArticleDataTable
					articles={workspace.articles}
					selectedArticle={workspace.selectedArticle}
					openArticle={workspace.openArticle}
				/>
			{/key}
			<PaginationLoadMore
				hasNextPage={workspace.articlesHasNextPage}
				isLoading={workspace.articlesLoadingMore}
				loadedCount={workspace.articles.length}
				label="articles"
				onLoadMore={workspace.loadMoreArticles}
			/>
		</div>
		<div class="flex min-h-0 flex-1 flex-col gap-3 overflow-auto md:hidden">
			{#each filtered as article (article.report_id)}
				<div
					class="rounded-lg border bg-card p-4 transition-colors data-[selected=true]:bg-primary/5"
					data-selected={workspace.selectedArticle === article.report_id}
				>
					<div class="flex items-start justify-between gap-3">
						<div class="min-w-0">
							<div class="font-medium break-words">
								{reportLabel(article)}
							</div>
							{#if article.doi}
								<div class="text-xs break-all text-muted-foreground">
									{article.doi}
								</div>
							{/if}
						</div>
						{#if workspace.selectedArticle === article.report_id}
							<Badge variant="default">Selected</Badge>
						{/if}
					</div>
					<div class="mt-3 flex flex-wrap gap-2">
						<Badge variant="outline">{article.issued_year ?? 'No year'}</Badge>
						<Badge variant="secondary">Total {article.total_citations}</Badge>
						<Badge variant="outline">Internal {article.internal_citations}</Badge>
						<Badge>Rank {article.rank_score.toFixed(2)}</Badge>
					</div>
					<Button
						class="mt-3 w-full"
						variant="outline"
						size="sm"
						onclick={() => workspace.openArticle(article.report_id)}
					>
						Open inspector
					</Button>
				</div>
			{:else}
				<Empty.Root class="min-h-32 border-dashed p-6">
					<Empty.Header>
						<Empty.Title>No articles match the current filters.</Empty.Title>
					</Empty.Header>
				</Empty.Root>
			{/each}
			<PaginationLoadMore
				hasNextPage={workspace.articlesHasNextPage}
				isLoading={workspace.articlesLoadingMore}
				loadedCount={workspace.articles.length}
				label="articles"
				onLoadMore={workspace.loadMoreArticles}
			/>
		</div>
	{/if}
</PageTemplate>
