<script lang="ts">
	import * as Alert from '$lib/components/ui/alert';
	import * as Empty from '$lib/components/ui/empty';
	import * as InputGroup from '$lib/components/ui/input-group';
	import * as Select from '$lib/components/ui/select';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { Slider } from '$lib/components/ui/slider';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import SearchIcon from '@lucide/svelte/icons/search';
	import ArticleDataTable from './articles-table/ArticleDataTable.svelte';
	import { useProjectWorkspaceContext, type ArticleSort } from './context.svelte.js';

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
					(article.doi.toLowerCase().includes(term) ||
						(article.title ?? '').toLowerCase().includes(term))
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
					return (a.title ?? a.doi).localeCompare(b.title ?? b.doi);
				}
				return b.rank_score - a.rank_score;
			})
	);
</script>

<div class="flex h-full min-h-0 flex-col gap-4 p-4">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold">Articles</h1>
			<p class="text-sm text-muted-foreground">
				{workspace.articles.length} project articles
			</p>
		</div>
		<Badge variant="secondary">{workspace.articles.length} articles</Badge>
	</div>

	<div class="grid gap-3 md:hidden">
		<InputGroup.Root>
			<InputGroup.Input
				placeholder="Search title or DOI"
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
			/>
			<Badge variant="outline">Min {workspace.articleFilters.minInternal}</Badge>
		</div>
	</div>

	{#if workspace.articlesError}
		<Alert.Root variant="destructive">
			<CircleAlertIcon />
			<Alert.Title>Articles unavailable</Alert.Title>
			<Alert.Description>{workspace.articlesError}</Alert.Description>
		</Alert.Root>
	{:else if workspace.articlesLoading}
		<div class="flex flex-col gap-2">
			{#each [0, 1, 2, 3, 4, 5, 6, 7] as index (index)}
				<Skeleton class="h-12" />
			{/each}
		</div>
	{:else if workspace.articles.length === 0}
		<Empty.Root class="min-h-80">
			<Empty.Header>
				<Empty.Title>No articles</Empty.Title>
				<Empty.Description>Start an ingestion to populate this project.</Empty.Description>
			</Empty.Header>
		</Empty.Root>
	{:else}
		<div class="hidden min-h-0 flex-1 md:flex">
			{#key workspace.selectedProjectId}
				<ArticleDataTable
					articles={workspace.articles}
					selectedArticle={workspace.selectedArticle}
					openArticle={workspace.openArticle}
				/>
			{/key}
		</div>
		<div class="flex min-h-0 flex-1 flex-col gap-3 overflow-auto md:hidden">
			{#each filtered as article (article.doi)}
				<section
					class="rounded-md border p-3"
					data-selected={workspace.selectedArticle === article.doi_key}
				>
					<div class="font-medium break-words">{article.title ?? article.doi}</div>
					<div class="text-xs break-all text-muted-foreground">{article.doi}</div>
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
						onclick={() => workspace.openArticle(article.doi_key)}
					>
						Open
					</Button>
				</section>
			{:else}
				<div class="rounded-md border p-6 text-center text-sm text-muted-foreground">
					No articles match the current filters.
				</div>
			{/each}
		</div>
	{/if}
</div>
