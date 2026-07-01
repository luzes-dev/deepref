<script lang="ts">
	import * as Card from '$lib/components/ui/card';
	import * as Table from '$lib/components/ui/table';
	import { Badge } from '$lib/components/ui/badge';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { statusVariant } from '$lib/api/helpers';
	import { useProjectWorkspaceContext } from './context.svelte.js';

	const workspace = useProjectWorkspaceContext();

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
	const loading = $derived(workspace.articlesLoading || workspace.ingestionsLoading);
</script>

<div class="flex h-full min-h-0 flex-col gap-4 p-4">
	<div class="flex flex-wrap items-start justify-between gap-3">
		<div class="min-w-0">
			<h1 class="truncate text-2xl font-semibold">{workspace.project.name}</h1>
			<p class="text-sm text-muted-foreground">{workspace.project.description}</p>
		</div>
		<Badge variant="secondary">Default depth {workspace.project.default_max_depth}</Badge>
	</div>

	{#if loading}
		<div class="grid gap-4 md:grid-cols-4">
			{#each [0, 1, 2, 3] as index (index)}
				<Skeleton class="h-28" />
			{/each}
		</div>
	{:else}
		<div class="grid gap-4 md:grid-cols-4">
			<Card.Root>
				<Card.Header><Card.Title>Articles</Card.Title></Card.Header>
				<Card.Content class="text-3xl font-semibold"
					>{workspace.articles.length}</Card.Content
				>
			</Card.Root>
			<Card.Root>
				<Card.Header><Card.Title>Internal citations</Card.Title></Card.Header>
				<Card.Content class="text-3xl font-semibold">{internalCitations}</Card.Content>
			</Card.Root>
			<Card.Root>
				<Card.Header><Card.Title>Total citations</Card.Title></Card.Header>
				<Card.Content class="text-3xl font-semibold">{totalCitations}</Card.Content>
			</Card.Root>
			<Card.Root>
				<Card.Header><Card.Title>Best rank</Card.Title></Card.Header>
				<Card.Content class="text-3xl font-semibold">{bestRank.toFixed(2)}</Card.Content>
			</Card.Root>
		</div>
	{/if}

	<section class="min-h-0 flex-1 rounded-md border">
		<div class="border-b p-4">
			<h2 class="font-medium">Recent ingestion activity</h2>
			<p class="text-sm text-muted-foreground">{recentIngestions.length} latest runs</p>
		</div>
		<Table.Root>
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
						<Table.Cell>{ingestion.seed_count}</Table.Cell>
						<Table.Cell>{ingestion.fetched_count}</Table.Cell>
						<Table.Cell>{new Date(ingestion.created_at).toLocaleString()}</Table.Cell>
					</Table.Row>
				{:else}
					<Table.Row>
						<Table.Cell colspan={4} class="h-28 text-center text-muted-foreground">
							No ingestion runs yet.
						</Table.Cell>
					</Table.Row>
				{/each}
			</Table.Body>
		</Table.Root>
	</section>
</div>
