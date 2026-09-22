<script lang="ts">
	import { resolve } from '$app/paths';
	import { createGetDependencyStatus } from '$lib/api/generated/health/health';
	import { statusVariant } from '$lib/api/helpers';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import * as Card from '@deepref/ui/card';
	import * as Table from '@deepref/ui/table';
	import * as Tabs from '@deepref/ui/tabs';
	import { PageHeader, StatePanel, Surface } from '@deepref/ui/layout';
	import ArrowRightIcon from '@lucide/svelte/icons/arrow-right';
	import BookOpenIcon from '@lucide/svelte/icons/book-open';
	import DatabaseIcon from '@lucide/svelte/icons/database';
	import ExternalLinkIcon from '@lucide/svelte/icons/external-link';
	import FileTextIcon from '@lucide/svelte/icons/file-text';
	import GitForkIcon from '@lucide/svelte/icons/git-fork';
	import NetworkIcon from '@lucide/svelte/icons/network';
	import SparklesIcon from '@lucide/svelte/icons/sparkles';
	import PageTemplate from '$lib/shell/PageTemplate.svelte';
	import { useProjectWorkspaceContext } from '../context.svelte.js';

	type OverviewState = 'error' | 'loading' | 'empty' | 'populated';

	let activeTab = $state('overview');

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
	const recentIngestions = $derived(workspace.ingestions.slice(0, 5));
	const dataError = $derived(workspace.articlesError ?? workspace.ingestionsError);
	const dependencyError = $derived(dependenciesQuery.error?.message);
	const loading = $derived(workspace.articlesLoading || workspace.ingestionsLoading);
	const hasEvidence = $derived(workspace.articles.length > 0 || workspace.ingestions.length > 0);
	const overviewState = $derived<OverviewState>(
		dataError && !hasEvidence
			? 'error'
			: loading && !hasEvidence
				? 'loading'
				: hasEvidence
					? 'populated'
					: 'empty'
	);
	const errorMessage = $derived(dataError ?? 'The evidence data could not be loaded.');

	function formatDate(value: string): string {
		return new Date(value).toLocaleString();
	}
</script>

<PageTemplate
	testId="overview-page"
	maxWidth="wide"
	role="region"
	aria-label="Overview content"
	data-overview-state={overviewState}
>
	<!-- Top Dashboard Header with Quick Actions -->
	<div class="flex flex-wrap items-center justify-between gap-4 border-b pb-5">
		<PageHeader
			title={workspace.project.name}
			description={workspace.project.description ??
				'A working summary of this evidence workspace.'}
		/>
		<div class="flex flex-wrap items-center gap-2">
			<Button
				variant="outline"
				size="sm"
				href={resolve('/projects/[projectId]/protocol', {
					projectId: workspace.selectedProjectId
				})}
			>
				<BookOpenIcon class="mr-1.5 size-4" />
				Review protocol
			</Button>
			<Button
				size="sm"
				href={resolve('/projects/[projectId]/screening/title-abstract', {
					projectId: workspace.selectedProjectId
				})}
			>
				Continue screening
				<ArrowRightIcon class="ml-1.5 size-4" />
			</Button>
		</div>
	</div>

	{#if dependencyError}
		<Surface
			as="section"
			tone="subtle"
			class="flex flex-wrap items-center justify-between gap-3 p-4"
			label="Dependency status warning"
		>
			<div class="min-w-0" aria-live="polite" data-testid="overview-dependency-warning">
				<p class="text-sm font-semibold text-foreground">Dependency status unavailable</p>
				<p class="mt-1 text-sm text-muted-foreground">
					Workspace evidence remains available, but service health could not be checked.
					{dependencyError}
				</p>
			</div>
			<Button
				variant="outline"
				size="sm"
				onclick={() => void dependenciesQuery.refetch()}
				disabled={dependenciesQuery.isFetching}
			>
				Refresh status
			</Button>
		</Surface>
	{/if}

	{#if dataError && hasEvidence}
		<Surface as="section" tone="subtle" class="p-4" label="Partial evidence warning">
			<div aria-live="polite" data-testid="overview-partial-data-warning">
				<p class="text-sm font-semibold text-foreground">
					Some evidence data is unavailable
				</p>
				<p class="mt-1 text-sm text-muted-foreground">
					Available workspace data is shown below. {dataError}
				</p>
			</div>
		</Surface>
	{/if}

	{#if overviewState === 'error'}
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
		<!-- Dashboard Tabs Pattern -->
		<Tabs.Root bind:value={activeTab} class="space-y-6">
			<div class="flex flex-wrap items-center justify-between gap-3">
				<Tabs.List>
					<Tabs.Trigger value="overview">Overview</Tabs.Trigger>
					<Tabs.Trigger value="imports"
						>Recent Imports ({workspace.ingestions.length})</Tabs.Trigger
					>
				</Tabs.List>
				<span class="text-xs text-muted-foreground">
					Evidence synchronized with local cache
				</span>
			</div>

			<Tabs.Content value="overview" class="space-y-6">
				<!-- KPI Metric Cards Grid (Dashboard Pattern) -->
				<section aria-labelledby="overview-metrics-title" data-testid="overview-populated">
					<div class="mb-3 flex items-baseline justify-between gap-3">
						<h2
							id="overview-metrics-title"
							class="text-sm font-semibold text-foreground"
						>
							Your library
						</h2>
						<span class="text-xs text-muted-foreground">Selected project</span>
					</div>
					<div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
						<Card.Root>
							<Card.Header class="flex flex-row items-center justify-between pb-2">
								<Card.Title class="text-sm font-medium">Articles</Card.Title>
								<FileTextIcon class="size-4 text-muted-foreground" />
							</Card.Header>
							<Card.Content>
								<div class="text-2xl font-bold [font-variant-numeric:tabular-nums]">
									{workspace.articles.length.toLocaleString()}
								</div>
								<p class="text-xs text-muted-foreground">articles collected</p>
							</Card.Content>
						</Card.Root>

						<Card.Root>
							<Card.Header class="flex flex-row items-center justify-between pb-2">
								<Card.Title class="text-sm font-medium"
									>Internal citations</Card.Title
								>
								<GitForkIcon class="size-4 text-muted-foreground" />
							</Card.Header>
							<Card.Content>
								<div class="text-2xl font-bold [font-variant-numeric:tabular-nums]">
									{internalCitations.toLocaleString()}
								</div>
								<p class="text-xs text-muted-foreground">
									connections between articles
								</p>
							</Card.Content>
						</Card.Root>

						<Card.Root>
							<Card.Header class="flex flex-row items-center justify-between pb-2">
								<Card.Title class="text-sm font-medium">Total citations</Card.Title>
								<NetworkIcon class="size-4 text-muted-foreground" />
							</Card.Header>
							<Card.Content>
								<div class="text-2xl font-bold [font-variant-numeric:tabular-nums]">
									{totalCitations.toLocaleString()}
								</div>
								<p class="text-xs text-muted-foreground">external + internal</p>
							</Card.Content>
						</Card.Root>

						<Card.Root>
							<Card.Header class="flex flex-row items-center justify-between pb-2">
								<Card.Title class="text-sm font-medium">Import Batches</Card.Title>
								<DatabaseIcon class="size-4 text-muted-foreground" />
							</Card.Header>
							<Card.Content>
								<div class="text-2xl font-bold [font-variant-numeric:tabular-nums]">
									{workspace.ingestions.length.toLocaleString()}
								</div>
								<p class="text-xs text-muted-foreground">
									completed ingestion runs
								</p>
							</Card.Content>
						</Card.Root>
					</div>
				</section>

				<!-- Two Column Dashboard Grid: Pipeline / Shortcuts & Recent Ingestions -->
				<div class="grid gap-6 lg:grid-cols-7">
					<!-- Left: Review Flow & Navigation Shortcuts -->
					<div class="flex flex-col gap-6 lg:col-span-4">
						<!-- Review Hero Banner -->
						<Card.Root class="overflow-hidden bg-card/60 backdrop-blur-sm">
							<Card.Header class="pb-3">
								<div
									class="flex items-center gap-2 text-xs font-semibold tracking-wider text-muted-foreground uppercase"
								>
									<SparklesIcon class="size-3.5 text-primary" />
									Continue your review
								</div>
								<Card.Title class="text-xl font-semibold">
									Review your collected articles
								</Card.Title>
								<Card.Description class="text-sm">
									Review titles and abstracts against your criteria, then read the
									full text of the papers you keep.
								</Card.Description>
							</Card.Header>
							<Card.Content class="pt-0">
								<div class="flex flex-wrap items-center gap-3">
									<Button
										href={resolve(
											'/projects/[projectId]/screening/title-abstract',
											{
												projectId: workspace.selectedProjectId
											}
										)}
									>
										Continue screening →
									</Button>
									<Button
										variant="outline"
										href={resolve('/projects/[projectId]/protocol', {
											projectId: workspace.selectedProjectId
										})}
									>
										Review protocol
									</Button>
								</div>
							</Card.Content>
						</Card.Root>

						<!-- Fast Nav Shortcuts Cards -->
						<div class="grid gap-3 sm:grid-cols-3">
							<a
								href={resolve('/projects/[projectId]/discovery/imports', {
									projectId: workspace.selectedProjectId
								})}
								class="group flex flex-col justify-between rounded-lg border bg-card p-4 transition-all hover:border-primary/50 hover:shadow-sm"
							>
								<div class="space-y-1">
									<div class="flex items-center justify-between">
										<span class="text-sm font-semibold group-hover:text-primary"
											>Add articles</span
										>
										<ExternalLinkIcon
											class="size-3.5 text-muted-foreground group-hover:text-primary"
										/>
									</div>
									<p class="text-xs text-muted-foreground">
										Import papers & references
									</p>
								</div>
							</a>

							<a
								href={resolve('/projects/[projectId]/graph', {
									projectId: workspace.selectedProjectId
								})}
								class="group flex flex-col justify-between rounded-lg border bg-card p-4 transition-all hover:border-primary/50 hover:shadow-sm"
							>
								<div class="space-y-1">
									<div class="flex items-center justify-between">
										<span class="text-sm font-semibold group-hover:text-primary"
											>Citation Graph</span
										>
										<ExternalLinkIcon
											class="size-3.5 text-muted-foreground group-hover:text-primary"
										/>
									</div>
									<p class="text-xs text-muted-foreground">
										Follow citation connections
									</p>
								</div>
							</a>

							<a
								href={resolve('/projects/[projectId]/prisma', {
									projectId: workspace.selectedProjectId
								})}
								class="group flex flex-col justify-between rounded-lg border bg-card p-4 transition-all hover:border-primary/50 hover:shadow-sm"
							>
								<div class="space-y-1">
									<div class="flex items-center justify-between">
										<span class="text-sm font-semibold group-hover:text-primary"
											>PRISMA Flow</span
										>
										<ExternalLinkIcon
											class="size-3.5 text-muted-foreground group-hover:text-primary"
										/>
									</div>
									<p class="text-xs text-muted-foreground">
										Review flow & export data
									</p>
								</div>
							</a>
						</div>
					</div>

					<!-- Right: Ingestions Activity Feed -->
					<div
						class="flex flex-col gap-6 lg:col-span-3"
						data-testid="overview-ingestions"
					>
						<Card.Root>
							<Card.Header
								class="flex flex-row items-center justify-between border-b pb-3"
							>
								<div>
									<Card.Title class="text-base font-semibold"
										>Recent imports</Card.Title
									>
									<Card.Description class="text-xs">
										{recentIngestions.length} latest {recentIngestions.length ===
										1
											? 'run'
											: 'runs'}
									</Card.Description>
								</div>
								<Badge variant="outline" class="text-xs">Import history</Badge>
							</Card.Header>
							<Card.Content class="p-0">
								<Table.Root containerLabel="Recent imports">
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
													<Badge
														variant={statusVariant(ingestion.status)}
													>
														{ingestion.status}
													</Badge>
												</Table.Cell>
												<Table.Cell
													class="[font-variant-numeric:tabular-nums]"
												>
													{ingestion.seed_count}
												</Table.Cell>
												<Table.Cell
													class="[font-variant-numeric:tabular-nums]"
												>
													{ingestion.fetched_count}
												</Table.Cell>
												<Table.Cell
													class="text-xs whitespace-nowrap text-muted-foreground"
												>
													{formatDate(ingestion.created_at)}
												</Table.Cell>
											</Table.Row>
										{:else}
											<Table.Row>
												<Table.Cell
													colspan={4}
													class="h-28 text-center text-xs text-muted-foreground"
												>
													No imports yet. Add articles from the Imports
													page.
												</Table.Cell>
											</Table.Row>
										{/each}
									</Table.Body>
								</Table.Root>
							</Card.Content>
						</Card.Root>
					</div>
				</div>
			</Tabs.Content>

			<Tabs.Content value="imports">
				<Card.Root>
					<Card.Header class="border-b pb-4">
						<div class="flex items-center justify-between">
							<div>
								<Card.Title>All Ingestions</Card.Title>
								<Card.Description>
									Complete list of ingestion runs conducted in this workspace
								</Card.Description>
							</div>
							<Button
								variant="outline"
								size="sm"
								href={resolve('/projects/[projectId]/discovery/imports', {
									projectId: workspace.selectedProjectId
								})}
							>
								Manage imports
							</Button>
						</div>
					</Card.Header>
					<Card.Content class="p-0">
						<Table.Root containerLabel="All workspace imports">
							<Table.Header>
								<Table.Row>
									<Table.Head>Status</Table.Head>
									<Table.Head>Seeds</Table.Head>
									<Table.Head>Fetched</Table.Head>
									<Table.Head>Created</Table.Head>
								</Table.Row>
							</Table.Header>
							<Table.Body>
								{#each workspace.ingestions as ingestion (ingestion.id)}
									<Table.Row>
										<Table.Cell>
											<Badge variant={statusVariant(ingestion.status)}>
												{ingestion.status}
											</Badge>
										</Table.Cell>
										<Table.Cell class="[font-variant-numeric:tabular-nums]">
											{ingestion.seed_count}
										</Table.Cell>
										<Table.Cell class="[font-variant-numeric:tabular-nums]">
											{ingestion.fetched_count}
										</Table.Cell>
										<Table.Cell
											class="text-xs whitespace-nowrap text-muted-foreground"
										>
											{formatDate(ingestion.created_at)}
										</Table.Cell>
									</Table.Row>
								{:else}
									<Table.Row>
										<Table.Cell
											colspan={4}
											class="h-28 text-center text-xs text-muted-foreground"
										>
											No imports recorded yet.
										</Table.Cell>
									</Table.Row>
								{/each}
							</Table.Body>
						</Table.Root>
					</Card.Content>
				</Card.Root>
			</Tabs.Content>
		</Tabs.Root>
	{/if}
</PageTemplate>
