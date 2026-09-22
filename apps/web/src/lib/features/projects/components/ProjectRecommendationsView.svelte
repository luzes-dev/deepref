<script lang="ts">
	import * as Tabs from '@deepref/ui/tabs';
	import { Badge } from '@deepref/ui/badge';
	import { PageToolbar, StatePanel, Surface } from '@deepref/ui/layout';
	import PageTemplate from '$lib/shell/PageTemplate.svelte';
	import GraphDegradedState from '$lib/features/projects/components/GraphDegradedState.svelte';
	import { createGetProjectRecommendations } from '$lib/api/generated/reports/reports';
	import type { RecommendationGroupsDto, ReportDto } from '$lib/api/generated/models';
	import { useProjectWorkspaceContext } from '../context.svelte.js';
	import { activeProjectQuery, createActiveProjectProjection } from '../project-queries.svelte';
	import { reportLabel } from '../report-label';

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

<PageTemplate testId="recommendations-page" maxWidth="default" tabindex="-1">
	<PageToolbar label="Recommendation projection status">
		<div class="flex flex-wrap items-center gap-2">
			<span class="text-sm text-muted-foreground"
				>{total} recommendations across 3 reading groups · Articles may appear in more than one
				group.</span
			>
		</div>
	</PageToolbar>

	<details class="disclosure">
		<summary>Recommendation update details</summary>
		<div class="flex flex-wrap gap-2">
			{#if recommendations.data}
				<Badge variant="secondary">Projection revision {groups.projection.revision}</Badge>
				<Badge variant="outline">Lag {groups.projection.lag}</Badge>
				{#if groups.projection.last_success_at}
					<Badge variant="outline">
						Projected {new Date(groups.projection.last_success_at).toLocaleString()}
					</Badge>
				{/if}
			{/if}
		</div>
	</details>
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
			<Tabs.Root value="foundational" class="gap-4">
				<Tabs.List
					variant="line"
					aria-label="Reading groups"
					class="max-w-full justify-start overflow-x-auto border-b"
				>
					{#each groupEntries as group (group.key)}<Tabs.Trigger value={group.key}
							>{group.label} ({group.articles.length})</Tabs.Trigger
						>{/each}
				</Tabs.List>
				{#each groupEntries as group (group.key)}
					<Tabs.Content value={group.key}>
						<div class="py-2">
							<h3 class="text-base font-semibold">{group.label}</h3>
							<p class="mt-1 text-sm text-muted-foreground">
								{group.description}
							</p>
							<Badge variant="outline" class="mt-3"
								>{group.articles.length} articles</Badge
							>
						</div>
						<div class="flex flex-col">
							{#each group.articles as article (article.report_id)}
								<button
									class="min-h-20 border-b border-border/60 bg-background py-4 text-left transition-colors hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring"
									onclick={() => workspace.openArticle(article.report_id)}
									aria-label={`Open ${reportLabel(article)}`}
									data-testid={`recommendation-${group.key}-${article.report_id}`}
								>
									<div class="line-clamp-3 leading-6 font-medium">
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
					</Tabs.Content>
				{/each}
			</Tabs.Root>
		</section>
	{/if}
</PageTemplate>
