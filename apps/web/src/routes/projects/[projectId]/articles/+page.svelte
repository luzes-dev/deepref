<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import * as Card from '$lib/components/ui/card';
	import * as Table from '$lib/components/ui/table';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Slider } from '$lib/components/ui/slider';
	import { createListProjectArticles } from '$lib/api/generated/articles/articles';

	let filter = $state('');
	let minInternal = $state(0);
	let sort = $state<'rank' | 'internal' | 'total' | 'year' | 'title'>('rank');
	const projectId = $derived(page.params.projectId ?? '');
	const articlesQuery = createListProjectArticles(
		() => projectId,
		() => ({ query: { staleTime: 0 } })
	);
	const articles = $derived(articlesQuery.data?.data ?? []);
	const error = $derived(articlesQuery.error?.message ?? '');

	const filtered = $derived(
		articles
			.filter((article) => {
				const term = filter.toLowerCase();
				return (
					article.internal_citations >= minInternal &&
					(article.doi.includes(term) ||
						(article.title ?? '').toLowerCase().includes(term))
				);
			})
			.toSorted((a, b) => {
				if (sort === 'internal') return b.internal_citations - a.internal_citations;
				if (sort === 'total') return b.total_citations - a.total_citations;
				if (sort === 'year') return (b.issued_year ?? 0) - (a.issued_year ?? 0);
				if (sort === 'title') return (a.title ?? a.doi).localeCompare(b.title ?? b.doi);
				return b.rank_score - a.rank_score;
			})
	);
</script>

<div class="flex flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold">Articles</h1>
		<p class="text-sm text-muted-foreground">
			Filter, sort, and open article-level citation details.
		</p>
	</div>
	<Card.Root>
		<Card.Header>
			<Card.Title>Filters</Card.Title>
			<Card.Description>{filtered.length} visible of {articles.length}</Card.Description>
		</Card.Header>
		<Card.Content class="grid gap-4 md:grid-cols-[1fr_220px_220px]">
			<Input placeholder="Search title or DOI" bind:value={filter} />
			<select class="h-9 rounded-md border bg-background px-3 text-sm" bind:value={sort}>
				<option value="rank">Rank score</option>
				<option value="internal">Internal citations</option>
				<option value="total">Total citations</option>
				<option value="year">Year</option>
				<option value="title">Title</option>
			</select>
			<div class="flex items-center gap-3">
				<Slider type="single" bind:value={minInternal} max={20} step={1} />
				<Badge variant="outline">Min {minInternal}</Badge>
			</div>
		</Card.Content>
	</Card.Root>
	{#if error}
		<Card.Root
			><Card.Content class="p-6 text-sm text-muted-foreground">{error}</Card.Content
			></Card.Root
		>
	{:else}
		<Card.Root>
			<Card.Content class="p-0">
				<Table.Root>
					<Table.Header>
						<Table.Row>
							<Table.Head>Article</Table.Head>
							<Table.Head>Year</Table.Head>
							<Table.Head>Total</Table.Head>
							<Table.Head>Internal</Table.Head>
							<Table.Head>Rank</Table.Head>
							<Table.Head></Table.Head>
						</Table.Row>
					</Table.Header>
					<Table.Body>
						{#each filtered as article (article.doi)}
							<Table.Row>
								<Table.Cell>
									<div class="font-medium">{article.title ?? article.doi}</div>
									<div class="text-xs text-muted-foreground">{article.doi}</div>
								</Table.Cell>
								<Table.Cell>{article.issued_year ?? '-'}</Table.Cell>
								<Table.Cell>{article.total_citations}</Table.Cell>
								<Table.Cell>{article.internal_citations}</Table.Cell>
								<Table.Cell>{article.rank_score.toFixed(2)}</Table.Cell>
								<Table.Cell class="text-right">
									<Button
										variant="outline"
										size="sm"
										href={resolve(
											`/projects/${projectId}/articles/${article.doi_key}`
										)}>Open</Button
									>
								</Table.Cell>
							</Table.Row>
						{/each}
					</Table.Body>
				</Table.Root>
			</Card.Content>
		</Card.Root>
	{/if}
</div>
