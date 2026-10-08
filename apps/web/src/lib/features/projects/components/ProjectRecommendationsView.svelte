<script lang="ts">
	import * as Tabs from '@deepref/ui/tabs';
	import { Input } from '@deepref/ui/input';
	import { Button } from '@deepref/ui/button';
	import { StatePanel } from '@deepref/ui/layout';
	import PageTemplate from '$lib/shell/PageTemplate.svelte';
	import GraphDegradedState from './GraphDegradedState.svelte';
	import { createGetProjectRecommendations } from '$lib/api/generated/reports/reports';
	import type { RecommendationGroupsDto } from '$lib/api/generated/models';
	import { useProjectWorkspaceContext } from '../context.svelte.js';
	import { activeProjectQuery, createActiveProjectProjection } from '../project-queries.svelte';
	import { reportLabel, reportSearchText } from '../report-label';

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
	const groupEntries = $derived([
		{
			key: 'foundational',
			label: 'Foundational',
			description: 'Articles in this review that other articles in it cite most.',
			articles: groups.foundational
		},
		{
			key: 'core_to_project',
			label: 'Core to project',
			description: "Highest-ranked articles in this review's citation network.",
			articles: groups.core_to_project
		},
		{
			key: 'underexplored',
			label: 'Underexplored',
			description: 'Articles that cite others in the review but are not cited yet.',
			articles: groups.underexplored
		}
	]);
	const total = $derived(groupEntries.reduce((sum, group) => sum + group.articles.length, 0));
	let search = $state('');
</script>

<PageTemplate
	testId="recommendations-page"
	maxWidth="full"
	scrollable={false}
	containerClass="min-h-0 gap-4 p-4 sm:p-4 lg:p-4"
>
	<header class="flex flex-wrap items-center justify-between gap-3">
		<div>
			<h2 class="text-base font-semibold">Find your next read</h2>
			<p class="mt-1 text-xs text-muted-foreground">
				Most connected articles in this review's citation network, excluding ones you
				excluded at screening · {total} shown · Articles can belong to multiple groups.
			</p>
		</div>
		<Input
			type="search"
			aria-label="Search recommendations"
			placeholder="Search recommendations"
			bind:value={search}
			class="w-full sm:w-64"
		/>
	</header>
	{#if recommendations.error}
		<GraphDegradedState
			error={recommendations.error}
			feature="Recommendations"
			projection={projectionQuery.data?.data}
			onRetry={() => void recommendations.refetch()}
		/>
	{:else if recommendations.isPending && enabled}
		<StatePanel
			state="loading"
			title="Finding related evidence"
			description="Ranking citation signals into recommendation groups."
		/>
	{:else if total === 0}
		<StatePanel
			state="empty"
			title="Not enough citation links yet"
			description="Recommendations rank how this review's articles cite each other. Imports at depth 0 don't fetch references, so there are no links to rank. Import with a higher depth to build the network."
		/>
	{:else}
		<section aria-label="Reading groups" class="min-h-0 flex-1 overflow-auto">
			<Tabs.Root value="foundational" class="gap-3">
				<Tabs.List
					variant="line"
					aria-label="Reading groups"
					class="h-auto max-w-full flex-wrap justify-start gap-y-1 overflow-visible group-data-horizontal/tabs:h-auto"
				>
					{#each groupEntries as group (group.key)}<Tabs.Trigger value={group.key}
							>{group.label} ({group.articles.length})</Tabs.Trigger
						>{/each}
				</Tabs.List>
				{#each groupEntries as group (group.key)}
					<Tabs.Content value={group.key}>
						<p class="py-2 text-sm text-muted-foreground">{group.description}</p>
						<div class="flex flex-col">
							{#each group.articles.filter( (article) => reportSearchText(article).includes(search.toLowerCase()) ) as article (article.report_id)}
								<button
									class="flex items-start gap-4 rounded-md px-3 py-2.5 text-left transition-colors hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring aria-pressed:bg-accent aria-pressed:shadow-inset-accent"
									onclick={() => workspace.openArticle(article.report_id)}
									aria-label={`Open ${reportLabel(article)}`}
									aria-pressed={workspace.selectedArticle === article.report_id}
									data-testid={`recommendation-${group.key}-${article.report_id}`}
								>
									<div class="min-w-0 flex-1">
										<div class="line-clamp-3 text-sm leading-6 font-medium">
											{reportLabel(article)}
										</div>
										<p class="mt-1 text-xs break-words text-muted-foreground">
											{article.issued_year ?? 'No year'}{#if article.doi}
												· {article.doi}{/if}
										</p>
									</div>
									<div
										class="shrink-0 text-right text-xs leading-6 text-muted-foreground tabular-nums"
									>
										<span class="font-medium text-foreground"
											>{article.total_citations}</span
										>
										citations · {article.internal_citations} internal
									</div>
								</button>
							{:else}<p class="py-8 text-sm text-muted-foreground">
									No articles match this search in {group.label.toLowerCase()}.
								</p>
								{#if search}<Button
										variant="ghost"
										size="sm"
										onclick={() => {
											search = '';
										}}>Clear search</Button
									>{/if}{/each}
						</div>
					</Tabs.Content>
				{/each}
			</Tabs.Root>
		</section>
	{/if}
	<details class="text-xs text-muted-foreground">
		<summary class="cursor-pointer">Recommendation update details</summary
		>{#if recommendations.data}<p class="mt-2">
				Projection revision {groups.projection.revision} · Lag {groups.projection
					.lag}{#if groups.projection.last_success_at}
					· Updated {new Date(groups.projection.last_success_at).toLocaleString()}{/if}
			</p>{/if}
	</details>
</PageTemplate>
