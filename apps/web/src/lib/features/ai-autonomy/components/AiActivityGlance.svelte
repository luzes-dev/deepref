<script lang="ts">
	import { resolve } from '$app/paths';
	import { createGetAiActivityOverview, createListAiActivity } from '#lib/api/generated/ai/ai.js';
	import { relativeTime } from '#lib/features/notifications/state.svelte.js';

	let { projectId }: { projectId: string } = $props();

	const overviewQuery = createGetAiActivityOverview(() => projectId);
	const recentQuery = createListAiActivity(
		() => projectId,
		() => ({ limit: 3, undone: false })
	);
	const overview = $derived(overviewQuery.data?.data);
	const recent = $derived(recentQuery.data?.data.items ?? []);
</script>

<section
	aria-labelledby="overview-ai-title"
	class="flex flex-col gap-3"
	data-testid="overview-ai-activity"
>
	<div class="flex items-baseline justify-between gap-3">
		<h3 id="overview-ai-title" class="font-semibold">AI activity</h3>
		<a
			class="inline-flex min-h-6 items-center text-muted-foreground hover:text-primary"
			href={resolve('/projects/[projectId]/activity', { projectId })}>See all</a
		>
	</div>
	{#if overview && (overview.to_verify > 0 || overview.open_conflicts > 0)}
		<p class="text-muted-foreground tabular-nums">
			{overview.to_verify} to verify · {overview.open_conflicts}
			{overview.open_conflicts === 1 ? 'conflict' : 'conflicts'}
		</p>
	{/if}
	<ul class="flex flex-col gap-2">
		{#each recent as entry (entry.id)}
			<li class="flex flex-col">
				<span class="line-clamp-2">{entry.summary}</span>
				<span class="text-xs text-muted-foreground">{relativeTime(entry.created_at)}</span>
			</li>
		{:else}
			<li class="text-muted-foreground">
				Nothing yet. What the AI does on its own will appear here, with undo.
			</li>
		{/each}
	</ul>
</section>
