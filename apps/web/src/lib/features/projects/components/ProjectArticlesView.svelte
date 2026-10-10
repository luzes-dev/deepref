<script lang="ts">
	import * as Alert from '@deepref/ui/alert';
	import * as Empty from '@deepref/ui/empty';
	import * as InputGroup from '@deepref/ui/input-group';
	import * as Select from '@deepref/ui/select';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import { Skeleton } from '@deepref/ui/skeleton';
	import { Slider } from '@deepref/ui/slider';
	import { Spinner } from '@deepref/ui/spinner';
	import PaginationLoadMore from '@deepref/ui/pagination-load-more';
	import { useQueryClient } from '@tanstack/svelte-query';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import SearchIcon from '@lucide/svelte/icons/search';
	import {
		getListProjectReportsQueryKey,
		recomputeProjectMetrics
	} from '#lib/api/generated/reports/reports.js';
	import { notifyError } from '#lib/features/notifications/toast.js';
	import PageTemplate from '#lib/shell/PageTemplate.svelte';
	import ArticleDataTable from './articles-table/ArticleDataTable.svelte';
	import { useProjectWorkspaceContext, type ArticleSort } from '../context.svelte.js';
	import { reportLabel, reportSearchText } from '../report-label';

	const workspace = useProjectWorkspaceContext();
	const queryClient = useQueryClient();

	// The worker recomputes citation counts in the background. Progress polls the
	// article list until counts exist, and gives up after a minute.
	const REFRESH_POLL_MS = 2_000;
	const REFRESH_TIMEOUT_MS = 60_000;

	let refreshing = $state(false);
	let refreshBaselineMs = $state(0);

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
	// An article is pending until its citation counts have been computed once.
	const pendingMetricsCount = $derived(
		workspace.articles.filter((article) => !article.metrics_as_of).length
	);
	const latestMetricsMs = $derived.by(() => {
		const values = workspace.articles
			.map((article) => article.metrics_as_of)
			.filter((value): value is string => Boolean(value))
			.map(Date.parse)
			.filter(Number.isFinite);
		return values.length > 0 ? Math.max(...values) : 0;
	});
	const latestMetricsAsOf = $derived(
		latestMetricsMs > 0 ? new Date(latestMetricsMs).toLocaleString() : undefined
	);
	const refreshComplete = $derived(
		pendingMetricsCount === 0 && latestMetricsMs > refreshBaselineMs
	);
	const refreshInProgress = $derived(refreshing && !refreshComplete);

	function refreshArticles(projectId: string) {
		return queryClient.invalidateQueries({
			queryKey: getListProjectReportsQueryKey(projectId)
		});
	}

	async function refreshMetrics() {
		const projectId = workspace.selectedProjectId;
		if (!projectId || refreshing) return;
		refreshBaselineMs = latestMetricsMs;
		refreshing = true;
		try {
			await recomputeProjectMetrics(projectId);
			await refreshArticles(projectId);
		} catch (error) {
			refreshing = false;
			notifyError('Citation counts could not be refreshed', error);
		}
	}

	$effect(() => {
		const projectId = workspace.selectedProjectId;
		if (!refreshInProgress || !projectId) return;
		const poll = setInterval(() => void refreshArticles(projectId), REFRESH_POLL_MS);
		const giveUp = setTimeout(() => (refreshing = false), REFRESH_TIMEOUT_MS);
		return () => {
			clearInterval(poll);
			clearTimeout(giveUp);
		};
	});
</script>

<PageTemplate
	testId="articles-page"
	maxWidth="full"
	scrollable={false}
	containerClass="min-h-0 gap-3 p-4 sm:p-4 lg:p-4"
>
	<p class="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
		<span class="tabular-nums"
			>{workspace.articles.length.toLocaleString()}
			{workspace.articlesHasNextPage ? 'articles loaded' : 'articles'}</span
		>
		{#if latestMetricsAsOf}<span aria-hidden="true">·</span><span
				>Metrics as of {latestMetricsAsOf}</span
			>{/if}
		{#if refreshInProgress}
			<span aria-hidden="true">·</span>
			<span
				class="flex items-center gap-1.5"
				role="status"
				data-testid="metrics-refresh-progress"><Spinner />Refreshing citation counts…</span
			>
		{:else if pendingMetricsCount > 0}
			<span aria-hidden="true">·</span>
			<span class="text-warning" data-testid="metrics-pending-banner"
				>Citation counts pending for {pendingMetricsCount.toLocaleString()}
				{pendingMetricsCount === 1 ? 'article' : 'articles'}</span
			>
		{/if}
		{#if pendingMetricsCount > 0 || staleMetrics.length > 0}
			<Button
				variant="outline"
				size="xs"
				data-testid="metrics-refresh"
				disabled={refreshing}
				onclick={() => void refreshMetrics()}
			>
				{#if refreshing}
					<Spinner data-icon="inline-start" />
				{:else}
					<RefreshCwIcon data-icon="inline-start" />
				{/if}
				{refreshing ? 'Refreshing…' : 'Refresh'}
			</Button>
		{/if}
	</p>

	<div class="grid gap-3 md:hidden" data-testid="article-mobile-filters">
		<InputGroup.Root>
			<InputGroup.Input
				placeholder="Search articles by title or DOI"
				aria-label="Search articles"
				bind:value={workspace.articleFilters.filter}
			/>
			<InputGroup.Addon><SearchIcon /></InputGroup.Addon>
		</InputGroup.Root>
		<details>
			<summary class="cursor-pointer text-xs text-muted-foreground">Sort & filter</summary>
			<div class="mt-3 flex flex-col gap-3">
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
		</details>
	</div>

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
			{#if workspace.articlesHasNextPage}
				<PaginationLoadMore
					hasNextPage={workspace.articlesHasNextPage}
					isLoading={workspace.articlesLoadingMore}
					loadedCount={workspace.articles.length}
					label="articles"
					onLoadMore={workspace.loadMoreArticles}
				/>
			{/if}
		</div>
		<div class="flex min-h-0 flex-1 flex-col gap-3 overflow-auto md:hidden">
			{#each filtered as article (article.report_id)}
				<button
					class="py-3 text-left transition-colors hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring aria-pressed:bg-muted"
					onclick={() => workspace.openArticle(article.report_id)}
					aria-pressed={workspace.selectedArticle === article.report_id}
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
					<p class="mt-2 text-xs text-muted-foreground tabular-nums">
						{article.issued_year ?? 'No year'} · {article.total_citations} citations · {article.internal_citations}
						internal · Rank {article.rank_score.toFixed(2)}
					</p>
				</button>
			{:else}
				<Empty.Root class="min-h-32 border-dashed p-6">
					<Empty.Header>
						<Empty.Title>No articles match the current filters.</Empty.Title>
					</Empty.Header>
				</Empty.Root>
			{/each}
			{#if workspace.articlesHasNextPage}
				<PaginationLoadMore
					hasNextPage={workspace.articlesHasNextPage}
					isLoading={workspace.articlesLoadingMore}
					loadedCount={workspace.articles.length}
					label="articles"
					onLoadMore={workspace.loadMoreArticles}
				/>
			{/if}
		</div>
	{/if}
</PageTemplate>
