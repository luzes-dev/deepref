<script lang="ts">
	import { Button } from '../../primitives/button';
	import { Spinner } from '../../primitives/spinner';

	// Only the action is shown: loaded counts and "all loaded" notes are pagination
	// bookkeeping that the list itself already communicates.
	let {
		hasNextPage,
		isLoading = false,
		onLoadMore
	}: {
		hasNextPage: boolean;
		isLoading?: boolean;
		/** @deprecated No longer rendered; kept so existing callers compile. */
		loadedCount?: number;
		/** @deprecated No longer rendered; kept so existing callers compile. */
		label?: string;
		onLoadMore: () => void;
	} = $props();
</script>

{#if hasNextPage}
	<div class="flex justify-center" data-testid="pagination-load-more">
		<Button variant="outline" onclick={onLoadMore} disabled={isLoading}>
			{#if isLoading}<Spinner data-icon="inline-start" />{/if}
			{isLoading ? 'Loading more' : 'Load more'}
		</Button>
	</div>
{/if}
