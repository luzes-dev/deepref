<script lang="ts">
	import { Button } from '@deepref/ui/button';
	import { Spinner } from '@deepref/ui/spinner';
	import { RefreshCw, Sparkles } from '@lucide/svelte';
	import type { ReviewRunObserver } from '../review-run-observer.svelte';

	let {
		run,
		allowRegenerate = false
	}: {
		run: ReviewRunObserver;
		/** Offer a fresh opinion; the caller knows whether the suggestion was rejected. */
		allowRegenerate?: boolean;
	} = $props();

	const showRetry = $derived(run.canRetry);
	const showRegenerate = $derived(allowRegenerate && run.canRegenerate);
</script>

{#if showRetry || showRegenerate}
	<div class="flex flex-wrap gap-2" data-testid="review-run-actions">
		{#if showRetry}
			<Button
				variant="outline"
				size="sm"
				disabled={run.restarting || run.isActive}
				onclick={() => void run.retry()}
			>
				{#if run.restarting}<Spinner data-icon="inline-start" />{:else}<RefreshCw
						data-icon="inline-start"
					/>{/if}
				Retry
			</Button>
		{/if}
		{#if showRegenerate}
			<Button
				variant="outline"
				size="sm"
				disabled={run.restarting || run.isActive}
				onclick={() => void run.regenerate()}
			>
				{#if run.restarting}<Spinner data-icon="inline-start" />{:else}<Sparkles
						data-icon="inline-start"
					/>{/if}
				Regenerate
			</Button>
		{/if}
	</div>
{/if}
