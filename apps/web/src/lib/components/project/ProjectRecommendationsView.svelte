<script lang="ts">
	import * as Empty from '$lib/components/ui/empty';
	import { Badge } from '$lib/components/ui/badge';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import GraphDegradedState from '$lib/components/GraphDegradedState.svelte';
	import { createGetProjectRecommendations } from '$lib/api/generated/reports/reports';
	import { createGetProjectProjection } from '$lib/api/generated/projection/projection';
	import type { RecommendationGroupsDto, ReportDto } from '$lib/api/generated/models';
	import { useProjectWorkspaceContext } from './context.svelte.js';
	import { reportLabel } from './report-label';

	const workspace = useProjectWorkspaceContext();
	const enabled = $derived(workspace.view === 'recommendations');

	const recommendations = createGetProjectRecommendations(
		() => workspace.project.id,
		() => ({ query: { enabled: Boolean(workspace.project.id && enabled), staleTime: 0 } })
	);
	const projectionQuery = createGetProjectProjection(
		() => workspace.project.id,
		() => ({ query: { enabled: Boolean(workspace.project.id && enabled), staleTime: 0 } })
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
		['foundational', groups.foundational],
		['core_to_project', groups.core_to_project],
		['underexplored', groups.underexplored]
	] as [string, ReportDto[]][]);
	const total = $derived(groupEntries.reduce((sum, [, articles]) => sum + articles.length, 0));
</script>

<div class="flex h-full min-h-0 flex-col gap-4 p-4">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold">Recommendations</h1>
			<p class="text-sm text-muted-foreground">
				Reading groups from project citation signals.
			</p>
		</div>
		<div class="flex flex-wrap gap-2">
			<Badge variant="secondary">{total} articles</Badge>
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
	</div>

	{#if recommendations.error}
		<GraphDegradedState
			error={recommendations.error}
			feature="Recommendations"
			projection={projectionQuery.data?.data}
			onRetry={() => void recommendations.refetch()}
		/>
	{:else if recommendations.isPending && enabled}
		<div class="grid gap-4 lg:grid-cols-3">
			{#each [0, 1, 2] as index (index)}
				<Skeleton class="h-80" />
			{/each}
		</div>
	{:else if total === 0}
		<Empty.Root class="min-h-80">
			<Empty.Header>
				<Empty.Title>No recommendations</Empty.Title>
				<Empty.Description
					>Recommendations appear after the project has enough article data.</Empty.Description
				>
			</Empty.Header>
		</Empty.Root>
	{:else}
		<div class="grid min-h-0 flex-1 gap-4 overflow-auto lg:grid-cols-3">
			{#each groupEntries as [name, articles] (name)}
				<section class="min-h-0 rounded-md border">
					<div class="border-b p-4">
						<h2 class="font-medium capitalize">{name.replaceAll('_', ' ')}</h2>
						<p class="text-sm text-muted-foreground">{articles.length} articles</p>
					</div>
					<div class="flex max-h-[calc(100vh-18rem)] flex-col gap-3 overflow-auto p-3">
						{#each articles as article (article.report_id)}
							<button
								class="rounded-md border p-3 text-left hover:bg-muted"
								onclick={() => workspace.openArticle(article.report_id)}
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
									<Badge variant="outline">Total {article.total_citations}</Badge>
								</div>
							</button>
						{/each}
					</div>
				</section>
			{/each}
		</div>
	{/if}
</div>
