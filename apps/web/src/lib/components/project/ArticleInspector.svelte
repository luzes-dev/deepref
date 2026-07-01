<script lang="ts">
	import * as Alert from '$lib/components/ui/alert';
	import * as Card from '$lib/components/ui/card';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { CopyButton } from '$lib/components/ui/copy-button';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { createGetProjectArticle } from '$lib/api/generated/articles/articles';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import XIcon from '@lucide/svelte/icons/x';
	import { useProjectWorkspaceContext } from './context.svelte.js';

	const workspace = useProjectWorkspaceContext();

	const articleQuery = createGetProjectArticle(
		() => workspace.project.id,
		() => workspace.selectedArticle ?? '',
		() => ({
			query: {
				enabled: Boolean(workspace.project.id && workspace.selectedArticle),
				staleTime: 0
			}
		})
	);
	const article = $derived(articleQuery.data?.data);
</script>

<aside class="flex h-full min-h-0 flex-col border-l bg-background">
	<div class="flex items-center justify-between gap-2 border-b p-4">
		<div class="min-w-0">
			<h2 class="truncate font-medium">Article inspector</h2>
			<p class="truncate text-xs text-muted-foreground">
				{workspace.selectedArticle ?? 'No article selected'}
			</p>
		</div>
		<Button
			variant="ghost"
			size="icon"
			onclick={workspace.clearArticle}
			aria-label="Clear article"
		>
			<XIcon data-icon />
		</Button>
	</div>

	<div class="min-h-0 flex-1 overflow-auto p-4">
		{#if !workspace.selectedArticle}
			<div
				class="flex h-full items-center justify-center text-center text-sm text-muted-foreground"
			>
				Select an article to inspect its metadata.
			</div>
		{:else if articleQuery.error}
			<Alert.Root variant="destructive">
				<CircleAlertIcon />
				<Alert.Title>Article unavailable</Alert.Title>
				<Alert.Description>{articleQuery.error.message}</Alert.Description>
				<Alert.Action onclick={workspace.clearArticle}>Clear selection</Alert.Action>
			</Alert.Root>
		{:else if articleQuery.isPending}
			<div class="flex flex-col gap-3">
				<Skeleton class="h-8 w-4/5" />
				<Skeleton class="h-4 w-2/3" />
				<Skeleton class="h-32" />
			</div>
		{:else if article}
			<div class="flex flex-col gap-4">
				<div class="flex items-start justify-between gap-3">
					<div class="min-w-0">
						<h3 class="break-words text-lg font-semibold">
							{article.title ?? article.doi}
						</h3>
						<p class="break-all text-sm text-muted-foreground">{article.doi}</p>
					</div>
					<CopyButton text={article.doi} />
				</div>

				<div class="grid grid-cols-2 gap-3">
					<Card.Root>
						<Card.Header><Card.Title>Total</Card.Title></Card.Header>
						<Card.Content class="text-2xl font-semibold"
							>{article.total_citations}</Card.Content
						>
					</Card.Root>
					<Card.Root>
						<Card.Header><Card.Title>References</Card.Title></Card.Header>
						<Card.Content class="text-2xl font-semibold"
							>{article.references_count}</Card.Content
						>
					</Card.Root>
					<Card.Root>
						<Card.Header><Card.Title>Year</Card.Title></Card.Header>
						<Card.Content class="text-2xl font-semibold"
							>{article.issued_year ?? '-'}</Card.Content
						>
					</Card.Root>
					<Card.Root>
						<Card.Header><Card.Title>Type</Card.Title></Card.Header>
						<Card.Content><Badge>{article.type ?? 'unknown'}</Badge></Card.Content>
					</Card.Root>
				</div>

				<section class="flex flex-col gap-2">
					<h4 class="font-medium">Metadata</h4>
					<p class="text-sm text-muted-foreground">{article.publisher}</p>
					<p class="break-words text-sm">
						{article.abstract ?? 'No abstract available.'}
					</p>
					<pre
						class="max-h-80 overflow-auto rounded-md bg-muted p-3 text-xs">{JSON.stringify(
							article.raw,
							null,
							2
						)}</pre>
				</section>
			</div>
		{/if}
	</div>
</aside>
