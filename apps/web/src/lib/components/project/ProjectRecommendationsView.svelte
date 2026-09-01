<script lang="ts">
	import { Badge } from '$lib/components/ui/badge';
	import { PageHeader, PageToolbar, StatePanel, Surface } from '$lib/components/layout';
	import GraphDegradedState from '$lib/components/GraphDegradedState.svelte';
	import { createGetProjectRecommendations } from '$lib/api/generated/reports/reports';
	import type { RecommendationGroupsDto, ReportDto } from '$lib/api/generated/models';
	import { useProjectWorkspaceContext } from './context.svelte.js';
	import { activeProjectQuery, createActiveProjectProjection } from './project-queries.svelte';
	import { reportLabel } from './report-label';

	const workspace = useProjectWorkspaceContext();
	const enabled = $derived(workspace.view === 'recommendations');

	const recommendations = createGetProjectRecommendations(
		() => workspace.project.id,
		() => activeProjectQuery(workspace.project.id, enabled)
	);
	const projectionQuery = createActiveProjectProjection(
		() => workspace.project.id,
		() => enabled
	);
	const groups = $derived<RecommendationGroupsDto>(
		recommendations.data?.data ?? {
			foundational: [],
			core_to_project: [],
			underexplored: [],
			projection: { revision: 0, lag: 0 }
		}
	);
	type RecommendationGroupKey = 'foundational' | 'core_to_project' | 'underexplored';
	type RecommendationGroup = {
		key: RecommendationGroupKey;
		label: string;
		description: string;
		articles: ReportDto[];
	};
	const groupEntries = $derived<RecommendationGroup[]>([
		{
			key: 'foundational',
			label: 'Foundational',
			description: 'Core works that establish the evidence base.',
			articles: groups.foundational
		},
		{
			key: 'core_to_project',
			label: 'Core to project',
			description: 'Highly connected works for this review question.',
			articles: groups.core_to_project
		},
		{
			key: 'underexplored',
			label: 'Underexplored',
			description: 'Promising links that may broaden the search.',
			articles: groups.underexplored
		}
	]);
	const total = $derived(groupEntries.reduce((sum, group) => sum + group.articles.length, 0));
</script>

<div
	class="flex h-full min-h-0 flex-col overflow-auto bg-background"
	tabindex="-1"
	data-testid="recommendations-page"
>
	<div
		class="mx-auto flex w-full max-w-[1440px] flex-1 flex-col gap-5 p-4 sm:gap-6 sm:p-6 lg:p-8"
	>
		<PageHeader
			eyebrow="Evidence workspace / Analysis"
			title="Recommendations"
			description="Reading groups from project citation signals."
		/>

		<PageToolbar label="Recommendation projection status">
			<div class="flex flex-wrap items-center gap-2">
				<Badge variant="secondary">{total} articles</Badge>
				{#if recommendations.data}
					<Badge variant="secondary"
						>Projection revision {groups.projection.revision}</Badge
					>
					<Badge variant="outline">Lag {groups.projection.lag}</Badge>
					{#if groups.projection.last_success_at}
						<Badge variant="outline">
							Projected {new Date(groups.projection.last_success_at).toLocaleString()}
						</Badge>
					{/if}
				{/if}
			</div>
		</PageToolbar>

		{#if recommendations.error}
			<GraphDegradedState
				error={recommendations.error}
				feature="Recommendations"
				projection={projectionQuery.data?.data}
				onRetry={() => void recommendations.refetch()}
			/>
		{:else if recommendations.isPending && enabled}
			<Surface as="section" tone="subtle" class="p-4 sm:p-6">
				<StatePanel
					state="loading"
					title="Finding related evidence"
					description="Ranking citation signals into recommendation groups."
				/>
			</Surface>
		{:else if total === 0}
			<Surface as="section" tone="subtle" class="p-4 sm:p-6">
				<StatePanel
					state="empty"
					title="No recommendations"
					description="Recommendations appear after the project has enough article data."
				/>
			</Surface>
		{:else}
			<section aria-labelledby="recommendation-groups-title" class="min-h-0 flex-1">
				<div class="mb-3 flex items-baseline justify-between gap-3">
					<h2
						id="recommendation-groups-title"
						class="text-sm font-semibold tracking-[0.08em] text-muted-foreground uppercase"
					>
						Reading groups
					</h2>
					<span class="text-xs text-muted-foreground"
						>Select an article to inspect evidence</span
					>
				</div>
				<div class="grid min-h-0 gap-4 overflow-auto lg:grid-cols-3">
					{#each groupEntries as group (group.key)}
						<Surface
							as="section"
							tone="default"
							class="min-h-0 overflow-hidden"
							label={group.label}
						>
							<div class="border-b border-border/70 p-4">
								<h3 class="text-base font-semibold">{group.label}</h3>
								<p class="mt-1 text-sm text-muted-foreground">
									{group.description}
								</p>
								<Badge variant="outline" class="mt-3"
									>{group.articles.length} articles</Badge
								>
							</div>
							<div
								class="flex max-h-[calc(100vh-22rem)] flex-col gap-3 overflow-auto p-3"
							>
								{#each group.articles as article (article.report_id)}
									<button
										class="min-h-20 rounded-md bg-background p-3 text-left ring-1 ring-border/80 transition-colors hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring"
										onclick={() => workspace.openArticle(article.report_id)}
										aria-label={`Open ${reportLabel(article)}`}
										data-testid={`recommendation-${group.key}-${article.report_id}`}
									>
										<div class="truncate font-medium">
											{reportLabel(article)}
										</div>
										{#if article.doi}
											<div class="text-xs break-all text-muted-foreground">
												{article.doi}
											</div>
										{/if}
										<div class="mt-2 flex flex-wrap gap-2">
											<Badge variant="secondary"
												>Internal {article.internal_citations}</Badge
											>
											<Badge variant="outline"
												>Total {article.total_citations}</Badge
											>
										</div>
									</button>
								{/each}
							</div>
						</Surface>
					{/each}
				</div>
			</section>
		{/if}
	</div>
</div>
