<script lang="ts">
	import type { Snippet } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { createGetAiActivityOverview } from '$lib/api/generated/ai/ai';
	import { Button } from '@deepref/ui/button';
	import { Badge } from '@deepref/ui/badge';
	import * as Alert from '@deepref/ui/alert';
	import ConflictsPanel from './ConflictsPanel.svelte';

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
	const conflictsView = $derived(page.url.searchParams.get('view') === 'conflicts');
	const open = $derived(overviewQuery.data?.data.open_conflicts ?? 0);
	// The server knows whether the AI second reviewer can run on its own here.
	const secondReview = $derived(
		stage === 'full_text'
			? overviewQuery.data?.data.second_review_full_text
			: overviewQuery.data?.data.second_review_title_abstract
	);

	async function show(view: 'queue' | 'conflicts'): Promise<void> {
		const url = new URL(page.url);
		if (view === 'conflicts') url.searchParams.set('view', 'conflicts');
		else url.searchParams.delete('view');
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- same-page query change
		await goto(url, { replaceState: true, keepFocus: true, noScroll: true });
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
			variant={conflictsView ? 'ghost' : 'secondary'}
			aria-current={conflictsView ? undefined : 'page'}
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
	</nav>
	{#if secondReview === 'needs_calibration' || secondReview === 'calibration_stale'}
		<div class="mx-auto w-full max-w-[1440px] shrink-0 px-4 pt-3 sm:px-6 lg:px-8">
			<Alert.Root data-testid="second-review-paused">
				<Alert.Title>Automatic AI second review is paused</Alert.Title>
				<Alert.Description>
					{#if secondReview === 'calibration_stale'}
						The approved AI calibration was made for an earlier version of this protocol
						or of the AI reviewer, so it no longer covers the current review. The AI
						answers only when you ask for a suggestion until an expert approves a new
						calibration for this protocol. Ask the person who runs this DeepRef
						deployment.
					{:else}
						This stage is set to an AI second reviewer, but this protocol has no
						approved AI calibration yet, so the AI answers only when you ask for a
						suggestion. An expert must approve a calibration for this protocol before
						the AI screens records on its own. Ask the person who runs this DeepRef
						deployment.
					{/if}
				</Alert.Description>
			</Alert.Root>
		</div>
	{/if}
	{#if conflictsView}
		<div class="mx-auto min-h-0 w-full max-w-[1440px] flex-1 overflow-y-auto p-4 sm:p-6 lg:p-8">
			<ConflictsPanel {projectId} {stage} />
		</div>
	{:else}
		<div class="flex min-h-0 flex-1 flex-col">
			{@render children()}
		</div>
	{/if}
</div>
