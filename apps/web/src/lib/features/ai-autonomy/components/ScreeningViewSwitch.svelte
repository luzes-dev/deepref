<script lang="ts">
	import type { Snippet } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { createGetAiActivityOverview } from '#lib/api/generated/ai/ai.js';
	import { Button } from '@deepref/ui/button';
	import { Badge } from '@deepref/ui/badge';
	import * as Alert from '@deepref/ui/alert';
	import ConflictsPanel from './ConflictsPanel.svelte';
	import AiFirstPanel from './AiFirstPanel.svelte';

	let {
		projectId,
		stage,
		children
	}: {
		projectId: string;
		stage: 'title_abstract' | 'full_text';
		children: Snippet;
	} = $props();

	const overviewQuery = createGetAiActivityOverview(() => projectId);
	const aiFirstView = $derived(
		stage === 'title_abstract' && page.url.searchParams.get('view') === 'ai-first'
	);
	const conflictsView = $derived(page.url.searchParams.get('view') === 'conflicts');
	const open = $derived(overviewQuery.data?.data.open_conflicts ?? 0);
	// The server knows whether the AI second reviewer can run on its own here.
	const secondReview = $derived(
		stage === 'full_text'
			? overviewQuery.data?.data.second_review_full_text
			: overviewQuery.data?.data.second_review_title_abstract
	);

	async function show(view: 'queue' | 'conflicts' | 'ai-first'): Promise<void> {
		const url = new URL(page.url.href);
		if (view !== 'queue') url.searchParams.set('view', view);
		else url.searchParams.delete('view');
		await goto(url, { replace: true, reset: false });
	}
</script>

<!--
	The screening workspace is height-bound: the view tabs keep their size and the
	queue, reader and decision bar share the remaining viewport height, so the
	decision bar stays on screen while the reading pane scrolls. A min-height
	parent would let the children grow with their content instead.
-->
<div class="flex h-full min-h-0 flex-1 flex-col">
	<nav
		class="mx-auto flex w-full max-w-[1440px] shrink-0 items-center gap-1 px-4 pt-4 sm:px-6 lg:px-8"
		aria-label="Screening views"
	>
		<Button
			size="sm"
			variant={conflictsView || aiFirstView ? 'ghost' : 'secondary'}
			aria-current={conflictsView || aiFirstView ? undefined : 'page'}
			onclick={() => void show('queue')}>Screening</Button
		>
		<Button
			size="sm"
			variant={conflictsView ? 'secondary' : 'ghost'}
			aria-current={conflictsView ? 'page' : undefined}
			data-testid="conflicts-tab"
			onclick={() => void show('conflicts')}
		>
			Conflicts
			{#if open > 0}<Badge variant="warning" size="sm">{open}</Badge>{/if}
		</Button>
		{#if stage === 'title_abstract'}
			<Button
				size="sm"
				variant={aiFirstView ? 'secondary' : 'ghost'}
				aria-current={aiFirstView ? 'page' : undefined}
				onclick={() => void show('ai-first')}>AI-first</Button
			>
		{/if}
	</nav>
	{#if secondReview === 'automatic'}
		<div class="mx-auto w-full max-w-[1440px] shrink-0 px-4 pt-3 sm:px-6 lg:px-8">
			<Alert.Root data-testid="second-review-advisory">
				<Alert.Title>AI second opinions are advisory</Alert.Title>
				<Alert.Description
					>Available from the first record. Each opinion stays hidden until your decision;
					you settle disagreements and retain all screening authority.</Alert.Description
				>
			</Alert.Root>
		</div>
	{/if}
	{#if aiFirstView}
		<div class="mx-auto min-h-0 w-full max-w-[1440px] flex-1 overflow-y-auto p-4 sm:p-6 lg:p-8">
			<AiFirstPanel {projectId} />
		</div>
	{:else if conflictsView}
		<div class="mx-auto min-h-0 w-full max-w-[1440px] flex-1 overflow-y-auto p-4 sm:p-6 lg:p-8">
			<ConflictsPanel {projectId} {stage} />
		</div>
	{:else}
		<div class="flex min-h-0 flex-1 flex-col">
			{@render children()}
		</div>
	{/if}
</div>
