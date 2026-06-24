<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import * as Card from '$lib/components/ui/card';
	import { Badge } from '$lib/components/ui/badge';
	import { createGetProjectRecommendations } from '$lib/api/generated/articles/articles';
	import type { RecommendationGroupsDto } from '$lib/api/generated/models';

	const projectId = $derived(page.params.projectId ?? '');
	const recommendations = createGetProjectRecommendations(
		() => projectId,
		() => ({ query: { staleTime: 0 } })
	);
	const groups = $derived<RecommendationGroupsDto>(
		recommendations.data?.data ?? {
			foundational: [],
			core_to_project: [],
			underexplored: []
		}
	);
</script>

<div class="flex flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold">Recommendations</h1>
		<p class="text-sm text-muted-foreground">
			Reading groups generated from project-local and general citation signals.
		</p>
	</div>
	{#if recommendations.error}
		<Card.Root>
			<Card.Content class="p-6 text-sm text-muted-foreground"
				>{recommendations.error.message}</Card.Content
			>
		</Card.Root>
	{/if}
	<div class="grid gap-4 lg:grid-cols-3">
		{#each Object.entries(groups) as [name, articles] (name)}
			<Card.Root>
				<Card.Header>
					<Card.Title>{name.replaceAll('_', ' ')}</Card.Title>
					<Card.Description>{articles.length} articles</Card.Description>
				</Card.Header>
				<Card.Content class="flex flex-col gap-3">
					{#each articles as article (article.doi)}
						<a
							class="rounded-md border p-3 hover:bg-muted"
							href={resolve(`/projects/${projectId}/articles/${article.doi_key}`)}
						>
							<div class="font-medium">{article.title ?? article.doi}</div>
							<div class="mt-2 flex gap-2">
								<Badge variant="secondary"
									>Internal {article.internal_citations}</Badge
								>
								<Badge variant="outline">Total {article.total_citations}</Badge>
							</div>
						</a>
					{/each}
				</Card.Content>
			</Card.Root>
		{/each}
	</div>
</div>
