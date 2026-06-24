<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import * as Card from '$lib/components/ui/card';
	import * as Tabs from '$lib/components/ui/tabs';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { createListProjectArticles } from '$lib/api/generated/articles/articles';
	import { createGetProject } from '$lib/api/generated/projects/projects';

	const projectId = $derived(page.params.projectId ?? '');
	const projectQueryResult = createGetProject(() => projectId);
	const articlesQueryResult = createListProjectArticles(
		() => projectId,
		() => ({ query: { staleTime: 0 } })
	);
	const project = $derived(projectQueryResult.data?.data);
	const articles = $derived(articlesQueryResult.data?.data ?? []);
	const error = $derived(
		projectQueryResult.error?.message ?? articlesQueryResult.error?.message ?? ''
	);
</script>

<div class="flex flex-col gap-6">
	{#if project}
		<div class="flex items-start justify-between gap-4">
			<div>
				<h1 class="text-2xl font-semibold">{project.name}</h1>
				<p class="text-sm text-muted-foreground">{project.description}</p>
			</div>
			<Badge variant="secondary">Default depth {project.default_max_depth}</Badge>
		</div>
		<Tabs.Root value="overview">
			<Tabs.List>
				<Tabs.Trigger value="overview">Overview</Tabs.Trigger>
				<Tabs.Trigger value="actions">Actions</Tabs.Trigger>
			</Tabs.List>
			<Tabs.Content value="overview">
				<div class="grid gap-4 md:grid-cols-4">
					<Card.Root>
						<Card.Header><Card.Title>Articles</Card.Title></Card.Header>
						<Card.Content class="text-3xl font-semibold">{articles.length}</Card.Content
						>
					</Card.Root>
					<Card.Root>
						<Card.Header><Card.Title>Internal citations</Card.Title></Card.Header>
						<Card.Content class="text-3xl font-semibold"
							>{articles.reduce(
								(sum, a) => sum + a.internal_citations,
								0
							)}</Card.Content
						>
					</Card.Root>
					<Card.Root>
						<Card.Header><Card.Title>Total citations</Card.Title></Card.Header>
						<Card.Content class="text-3xl font-semibold"
							>{articles.reduce((sum, a) => sum + a.total_citations, 0)}</Card.Content
						>
					</Card.Root>
					<Card.Root>
						<Card.Header><Card.Title>Best rank</Card.Title></Card.Header>
						<Card.Content class="text-3xl font-semibold"
							>{Math.max(0, ...articles.map((a) => a.rank_score)).toFixed(
								2
							)}</Card.Content
						>
					</Card.Root>
				</div>
			</Tabs.Content>
			<Tabs.Content value="actions">
				<Card.Root>
					<Card.Header>
						<Card.Title>Project views</Card.Title>
						<Card.Description>Open the working views for this project.</Card.Description
						>
					</Card.Header>
					<Card.Content class="flex flex-wrap gap-2">
						<Button href={resolve(`/projects/${projectId}/articles`)}>Articles</Button>
						<Button href={resolve(`/projects/${projectId}/graph`)} variant="outline"
							>Graph</Button
						>
						<Button
							href={resolve(`/projects/${projectId}/recommendations`)}
							variant="outline">Recommendations</Button
						>
					</Card.Content>
				</Card.Root>
			</Tabs.Content>
		</Tabs.Root>
	{:else if error}
		<Card.Root
			><Card.Content class="p-6 text-sm text-muted-foreground">{error}</Card.Content
			></Card.Root
		>
	{:else}
		<Card.Root><Card.Content class="p-6">Loading project...</Card.Content></Card.Root>
	{/if}
</div>
