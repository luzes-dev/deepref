<script lang="ts">
	import * as Alert from '$lib/components/ui/alert';
	import * as Empty from '$lib/components/ui/empty';
	import * as InputGroup from '$lib/components/ui/input-group';
	import * as Select from '$lib/components/ui/select';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { Slider } from '$lib/components/ui/slider';
	import PaginationLoadMore from '$lib/components/PaginationLoadMore.svelte';
	import { MetricTile, PageHeader, PageToolbar, Surface } from '$lib/components/layout';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import SearchIcon from '@lucide/svelte/icons/search';
	import ArticleDataTable from './articles-table/ArticleDataTable.svelte';
	import { useProjectWorkspaceContext, type ArticleSort } from './context.svelte.js';
	import { reportLabel, reportSearchText } from './report-label';

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

<div class="flex h-full min-h-0 flex-col overflow-auto bg-background" data-testid="articles-page">
	<div class="mx-auto flex w-full max-w-[1440px] flex-col gap-5 p-4 sm:gap-6 sm:p-6 lg:p-8">
		<PageHeader
			eyebrow="Evidence workspace / Collect"
			title="Articles"
			description="Browse the evidence corpus, filter records, and inspect source provenance."
		>
			<p class="mt-2 text-xs text-muted-foreground">
				{workspace.articles.length.toLocaleString()} project articles
			</p>
		</PageHeader>

		<PageToolbar label="Article corpus status">
			<div class="flex flex-wrap items-center gap-2">
				<Badge variant="secondary">{workspace.articles.length} articles</Badge>
				<Badge variant={staleMetrics.length > 0 ? 'outline' : 'default'}>
					{staleMetrics.length > 0
						? `${staleMetrics.length} metrics stale`
						: 'Metrics current'}
				</Badge>
			</div>
		</PageToolbar>

		<div class="grid gap-3 sm:grid-cols-3" data-testid="article-metrics">
			<MetricTile
				label="Corpus"
				value={workspace.articles.length.toLocaleString()}
				detail="records loaded"
				tone="info"
				class="[font-variant-numeric:tabular-nums]"
			/>
			<MetricTile
				label="Stale metrics"
				value={staleMetrics.length.toLocaleString()}
				detail="awaiting projection"
				tone={staleMetrics.length > 0 ? 'warning' : 'positive'}
				class="[font-variant-numeric:tabular-nums]"
			/>
			<MetricTile
				label="Selected"
				value={workspace.selectedArticle ? '1' : '0'}
				detail="article inspector"
				tone="default"
				class="[font-variant-numeric:tabular-nums]"
			/>
		</div>

		<div class="grid gap-3 md:hidden" data-testid="article-mobile-filters">
			<InputGroup.Root>
				<InputGroup.Input
					placeholder="Search title, DOI, or report ID"
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
					{staleMetrics.length} loaded article metrics are awaiting graph projection. Metrics
					as of {latestMetricsAsOf ?? 'not yet computed'}.
				</Alert.Description>
			</Alert.Root>
		{:else if latestMetricsAsOf}
			<p class="text-sm text-muted-foreground">Metrics as of {latestMetricsAsOf}</p>
		{/if}

		{#if workspace.articlesError}
			<Alert.Root variant="destructive">
				<CircleAlertIcon />
				<Alert.Title>Articles unavailable</Alert.Title>
				<Alert.Description>{workspace.articlesError}</Alert.Description>
			</Alert.Root>
		{:else if workspace.articlesLoading}
			<Surface as="section" tone="subtle" class="p-4" label="Loading articles">
				<div class="flex flex-col gap-2" aria-label="Loading articles">
					{#each [0, 1, 2, 3, 4, 5, 6, 7] as index (index)}
						<Skeleton class="h-12" />
					{/each}
				</div>
			</Surface>
		{:else if workspace.articles.length === 0}
			<Surface as="section" tone="subtle" class="p-4 sm:p-6">
				<Empty.Root class="min-h-80 border-dashed">
					<Empty.Header>
						<Empty.Title>No articles</Empty.Title>
						<Empty.Description
							>Start an ingestion to populate this project.</Empty.Description
						>
					</Empty.Header>
				</Empty.Root>
			</Surface>
		{:else}
			<Surface
				as="section"
				tone="default"
				class="hidden min-h-0 flex-1 flex-col gap-3 p-4 md:flex"
			>
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
			</Surface>
			<div class="flex min-h-0 flex-1 flex-col gap-3 overflow-auto md:hidden">
				{#each filtered as article (article.report_id)}
					<div
						class="rounded-lg transition-colors data-[selected=true]:bg-primary/5"
						data-selected={workspace.selectedArticle === article.report_id}
					>
						<Surface
							as="article"
							tone="default"
							class="p-4"
							label={reportLabel(article)}
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
								<Badge variant="outline"
									>Internal {article.internal_citations}</Badge
								>
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
						</Surface>
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
	</div>
</div>
