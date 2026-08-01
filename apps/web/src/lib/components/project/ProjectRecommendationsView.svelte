<script lang="ts">
	import * as Alert from '$lib/components/ui/alert';
	import * as Empty from '$lib/components/ui/empty';
	import { Badge } from '$lib/components/ui/badge';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { createGetProjectRecommendations } from '$lib/api/generated/articles/articles';
	import type { ArticleDto, RecommendationGroupsDto } from '$lib/api/generated/models';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import { useProjectWorkspaceContext } from './context.svelte.js';

	const workspace = useProjectWorkspaceContext();
	const enabled = $derived(workspace.view === 'recommendations');

	const recommendations = createGetProjectRecommendations(
		() => workspace.project.id,
		() => ({ query: { enabled: Boolean(workspace.project.id && enabled), staleTime: 0 } })
	);
	const groups = $derived<RecommendationGroupsDto>(
		recommendations.data?.data ?? {
			foundational: [],
			core_to_project: [],
			underexplored: []
		}
	);
	const groupEntries = $derived([
		['foundational', groups.foundational],
		['core_to_project', groups.core_to_project],
		['underexplored', groups.underexplored]
	] as [string, ArticleDto[]][]);
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
		<Badge variant="secondary">{total} articles</Badge>
	</div>

	{#if recommendations.error}
		<Alert.Root variant="destructive">
			<CircleAlertIcon />
			<Alert.Title>Recommendations unavailable</Alert.Title>
			<Alert.Description>{recommendations.error.message}</Alert.Description>
		</Alert.Root>
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
						{#each articles as article (article.doi)}
							<button
								class="rounded-md border p-3 text-left hover:bg-muted"
								onclick={() => workspace.openArticle(article.doi_key)}
							>
								<div class="truncate font-medium">
									{article.title ?? article.doi}
								</div>
								<div class="break-all text-xs text-muted-foreground">
									{article.doi}
								</div>
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
