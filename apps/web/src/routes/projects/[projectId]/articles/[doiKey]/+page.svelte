<script lang="ts">
	import { page } from '$app/state';
	import * as Card from '$lib/components/ui/card';
	import { Badge } from '$lib/components/ui/badge';
	import { CopyButton } from '$lib/components/ui/copy-button';
	import { createGetProjectArticle } from '$lib/api/generated/articles/articles';

	const projectId = $derived(page.params.projectId ?? '');
	const doiKey = $derived(page.params.doiKey ?? '');
	const articleQueryResult = createGetProjectArticle(
		() => projectId,
		() => doiKey,
		() => ({ query: { staleTime: 0 } })
	);
	const article = $derived(articleQueryResult.data?.data);
	const error = $derived(articleQueryResult.error?.message ?? '');
</script>

{#if article}
	<div class="flex flex-col gap-6">
		<div class="flex items-start justify-between gap-4">
			<div>
				<h1 class="text-2xl font-semibold">{article.title ?? article.doi}</h1>
				<p class="text-sm text-muted-foreground">{article.doi}</p>
			</div>
			<CopyButton text={article.doi} />
		</div>
		<div class="grid gap-4 md:grid-cols-4">
			<Card.Root
				><Card.Header><Card.Title>Total citations</Card.Title></Card.Header><Card.Content
					class="text-3xl font-semibold">{article.total_citations}</Card.Content
				></Card.Root
			>
			<Card.Root
				><Card.Header><Card.Title>References</Card.Title></Card.Header><Card.Content
					class="text-3xl font-semibold">{article.references_count}</Card.Content
				></Card.Root
			>
			<Card.Root
				><Card.Header><Card.Title>Year</Card.Title></Card.Header><Card.Content
					class="text-3xl font-semibold">{article.issued_year ?? '-'}</Card.Content
				></Card.Root
			>
			<Card.Root
				><Card.Header><Card.Title>Type</Card.Title></Card.Header><Card.Content
					><Badge>{article.type ?? 'unknown'}</Badge></Card.Content
				></Card.Root
			>
		</div>
		<Card.Root>
			<Card.Header
				><Card.Title>Metadata</Card.Title><Card.Description
					>{article.publisher}</Card.Description
				></Card.Header
			>
			<Card.Content class="flex flex-col gap-3 text-sm">
				<p>{article.abstract ?? 'No abstract available.'}</p>
				<pre class="overflow-auto rounded-md bg-muted p-3 text-xs">{JSON.stringify(
						article.raw,
						null,
						2
					)}</pre>
			</Card.Content>
		</Card.Root>
	</div>
{:else}
	<Card.Root
		><Card.Content class="p-6 text-sm text-muted-foreground"
			>{error || 'Loading article...'}</Card.Content
		></Card.Root
	>
{/if}
