<script lang="ts">
	import { Button } from '../../primitives/button';
	import { Spinner } from '../../primitives/spinner';

	let {
		hasNextPage,
		isLoading = false,
		loadedCount,
		label = 'items',
		onLoadMore
	}: {
		hasNextPage: boolean;
		isLoading?: boolean;
		loadedCount: number;
		label?: string;
		onLoadMore: () => void;
	} = $props();
</script>

<div class="flex flex-wrap items-center justify-between gap-3" data-testid="pagination-load-more">
	<p class="text-sm text-muted-foreground">Loaded {loadedCount}</p>
	{#if hasNextPage}
		<Button variant="outline" onclick={onLoadMore} disabled={isLoading}>
			{#if isLoading}<Spinner data-icon="inline-start" />{/if}
			{isLoading ? 'Loading more' : 'Load more'}
		</Button>
	{:else}
		<p class="text-sm text-muted-foreground">All {label} loaded</p>
	{/if}
</div>
