<script lang="ts">
	import { resolve } from '$app/paths';
	import { createGetDependencyStatus } from '$lib/api/generated/health/health';
	import { statusVariant } from '$lib/api/helpers';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Table from '$lib/components/ui/table';
	import {
		MetricTile,
		PageHeader,
		PageToolbar,
		StatePanel,
		Surface
	} from '$lib/components/layout';
	import { useProjectWorkspaceContext } from './context.svelte.js';

	type OverviewState = 'dependency-error' | 'error' | 'loading' | 'empty' | 'populated';

	const workspace = useProjectWorkspaceContext();
	const dependenciesQuery = createGetDependencyStatus(() => ({
		query: {
			staleTime: 5_000,
			refetchOnWindowFocus: 'always'
		}
	}));

	const internalCitations = $derived(
		workspace.articles.reduce((sum, article) => sum + article.internal_citations, 0)
	);
	const totalCitations = $derived(
		workspace.articles.reduce((sum, article) => sum + article.total_citations, 0)
	);
	const bestRank = $derived(
		Math.max(0, ...workspace.articles.map((article) => article.rank_score))
	);
	const recentIngestions = $derived(workspace.ingestions.slice(0, 5));
	const dataError = $derived(workspace.articlesError ?? workspace.ingestionsError);
	const dependencyError = $derived(dependenciesQuery.error?.message);
	const loading = $derived(workspace.articlesLoading || workspace.ingestionsLoading);
	const hasEvidence = $derived(workspace.articles.length > 0 || workspace.ingestions.length > 0);
	const overviewState = $derived<OverviewState>(
		dependencyError
			? 'dependency-error'
			: dataError
				? 'error'
				: loading
					? 'loading'
					: hasEvidence
						? 'populated'
						: 'empty'
	);
	const errorMessage = $derived(
		dependencyError ?? dataError ?? 'The evidence data could not be loaded.'
	);

	function formatDate(value: string): string {
		return new Date(value).toLocaleString();
	}
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div
	class="flex h-full min-h-0 flex-col overflow-auto bg-background"
	tabindex="0"
	role="region"
	aria-label="Overview content"
	data-testid="overview-page"
	data-overview-state={overviewState}
>
	<div class="mx-auto flex w-full max-w-[1440px] flex-col gap-5 p-4 sm:gap-6 sm:p-6 lg:p-8">
		<PageHeader
			eyebrow="Evidence workspace / Overview"
			title={workspace.project.name}
			description={workspace.project.description ??
				'A working summary of this evidence workspace.'}
		/>

		<PageToolbar label="Overview status">
			<div class="flex flex-wrap items-center gap-2">
				<Badge variant="secondary"
					>{workspace.articles.length.toLocaleString()} articles</Badge
				>
				<Badge variant="outline"
					>{workspace.ingestions.length.toLocaleString()} ingestion runs</Badge
				>
				<Badge variant="outline">Depth {workspace.project.default_max_depth}</Badge>
			</div>
		</PageToolbar>

		{#if overviewState === 'dependency-error'}
			<div data-testid="overview-dependency-error">
				<Surface as="section" tone="subtle" class="p-4 sm:p-6">
					<StatePanel
						state="error"
						title="Dependency status unavailable"
						description={`The last known workspace data is preserved, but service health could not be checked. ${errorMessage}`}
					>
						{#snippet action()}
							<Button
								variant="outline"
								size="sm"
								onclick={() => void dependenciesQuery.refetch()}
								disabled={dependenciesQuery.isFetching}
							>
								Refresh status
							</Button>
						{/snippet}
					</StatePanel>
				</Surface>
			</div>
		{:else if overviewState === 'error'}
			<div data-testid="overview-data-error">
				<Surface as="section" tone="subtle" class="p-4 sm:p-6">
					<StatePanel
						state="error"
						title="Evidence data unavailable"
						description={errorMessage}
					/>
				</Surface>
			</div>
		{:else if overviewState === 'loading'}
			<div data-testid="overview-loading">
				<Surface as="section" tone="subtle" class="p-4 sm:p-6">
					<StatePanel
						state="loading"
						title="Gathering workspace evidence"
						description="Article metrics and ingestion activity are being assembled."
					/>
				</Surface>
			</div>
		{:else if overviewState === 'empty'}
			<div data-testid="overview-empty">
				<Surface as="section" tone="subtle" class="p-4 sm:p-6">
					<StatePanel
						state="empty"
						title="No evidence in this workspace yet"
						description="Import source records to begin building an auditable evidence corpus."
					>
						{#snippet action()}
							<Button
								href={resolve('/projects/[projectId]/discovery/imports', {
									projectId: workspace.selectedProjectId
								})}
							>
								Open imports
							</Button>
						{/snippet}
					</StatePanel>
				</Surface>
			</div>
		{:else}
			<section aria-labelledby="overview-metrics-title" data-testid="overview-populated">
				<div class="mb-3 flex items-baseline justify-between gap-3">
					<h2
						id="overview-metrics-title"
						class="text-sm font-semibold tracking-[0.08em] text-muted-foreground uppercase"
					>
						Corpus at a glance
					</h2>
					<span class="text-xs text-muted-foreground">Selected project</span>
				</div>
				<div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
					<MetricTile
						label="Articles"
						value={workspace.articles.length.toLocaleString()}
						detail="records in corpus"
						tone="info"
						class="[font-variant-numeric:tabular-nums]"
					/>
					<MetricTile
						label="Internal citations"
						value={internalCitations.toLocaleString()}
						detail="links within corpus"
						tone="positive"
						class="[font-variant-numeric:tabular-nums]"
					/>
					<MetricTile
						label="Total citations"
						value={totalCitations.toLocaleString()}
						detail="external + internal"
						tone="warning"
						class="[font-variant-numeric:tabular-nums]"
					/>
					<MetricTile
						label="Best rank"
						value={bestRank.toFixed(2)}
						detail="evidence relevance"
						tone="positive"
						class="[font-variant-numeric:tabular-nums]"
					/>
				</div>
			</section>

			<div data-testid="overview-ingestions">
				<Surface as="section" tone="default" class="min-h-0 overflow-hidden">
					<div
						class="flex flex-wrap items-start justify-between gap-3 border-b border-border/70 p-4 sm:p-5"
					>
						<div>
							<h2 class="text-lg font-semibold">Recent ingestion activity</h2>
							<p class="mt-1 text-sm text-muted-foreground">
								{recentIngestions.length} latest {recentIngestions.length === 1
									? 'run'
									: 'runs'}
							</p>
						</div>
						<Badge variant="outline">Auditable history</Badge>
					</div>
					<Table.Root containerLabel="Recent ingestion activity">
						<Table.Header>
							<Table.Row>
								<Table.Head>Status</Table.Head>
								<Table.Head>Seeds</Table.Head>
								<Table.Head>Fetched</Table.Head>
								<Table.Head>Updated</Table.Head>
							</Table.Row>
						</Table.Header>
						<Table.Body>
							{#each recentIngestions as ingestion (ingestion.id)}
								<Table.Row>
									<Table.Cell>
										<Badge variant={statusVariant(ingestion.status)}
											>{ingestion.status}</Badge
										>
									</Table.Cell>
									<Table.Cell class="[font-variant-numeric:tabular-nums]"
										>{ingestion.seed_count}</Table.Cell
									>
									<Table.Cell class="[font-variant-numeric:tabular-nums]"
										>{ingestion.fetched_count}</Table.Cell
									>
									<Table.Cell class="whitespace-nowrap"
										>{formatDate(ingestion.created_at)}</Table.Cell
									>
								</Table.Row>
							{:else}
								<Table.Row>
									<Table.Cell
										colspan={4}
										class="h-28 text-center text-muted-foreground"
									>
										No ingestion runs yet.
									</Table.Cell>
								</Table.Row>
							{/each}
						</Table.Body>
					</Table.Root>
				</Surface>
			</div>
		{/if}
	</div>
</div>
