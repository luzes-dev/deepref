<script lang="ts">
	import { onMount } from 'svelte';
	import ReviewRunActions from '#lib/features/ai-assistance/components/ReviewRunActions.svelte';
	import { ReviewRunObserver } from '#lib/features/ai-assistance/review-run-observer.svelte.js';
	import type { ReviewRunDto } from '#lib/api/generated/models/index.js';

	let {
		projectId,
		run,
		allowRegenerate = false
	}: { projectId: string; run: ReviewRunDto; allowRegenerate?: boolean } = $props();

	const observer = new ReviewRunObserver(
		() => projectId,
		() => undefined
	);
	onMount(() => {
		void observer.observe(run);
		return () => observer.reset();
	});
</script>

<p data-testid="observed-run">{observer.current?.id ?? ''}:{observer.current?.state.kind ?? ''}</p>
<p data-testid="observed-error">{observer.error}</p>
<ReviewRunActions run={observer} {allowRegenerate} />
