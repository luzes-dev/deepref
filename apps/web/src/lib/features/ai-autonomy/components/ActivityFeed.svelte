<script lang="ts">
	import { createInfiniteQuery, useQueryClient } from '@tanstack/svelte-query';
	import { Bot, Sparkles, Undo2, Workflow } from '@lucide/svelte';
	import { resolve } from '$app/paths';
	import {
		createGetAiActivityOverview,
		createGetAiAutonomy,
		createUndoAiActivity,
		createUndoAiActivityBatch,
		listAiActivity
	} from '$lib/api/generated/ai/ai';
	import type { AiActivityDto } from '$lib/api/generated/models';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import { Skeleton } from '@deepref/ui/skeleton';
	import * as Select from '@deepref/ui/select';
	import { Switch } from '@deepref/ui/switch';
	import { StatePanel } from '@deepref/ui/layout';
	import { notifyError, notifyInfo, notifySuccess } from '$lib/features/notifications/toast';
	import { relativeTime } from '$lib/features/notifications/state.svelte';
	import PageTemplate from '$lib/shell/PageTemplate.svelte';
	import { ACTOR_LABEL, activityKeys, groupActivity, undoableCount } from '../activity';

	let { projectId }: { projectId: string } = $props();

	const queryClient = useQueryClient();
	const autonomyQuery = createGetAiAutonomy(() => projectId);
	const overviewQuery = createGetAiActivityOverview(() => projectId);
	const undoMutation = createUndoAiActivity();
	const undoBatchMutation = createUndoAiActivityBatch();

	let task = $state('all');
	let actorType = $state('all');
	let showUndone = $state(false);
	let confirmingBatch = $state<string | null>(null);
	let busyId = $state<string | null>(null);

	const taskOptions = $derived([
		{ value: 'all', label: 'All kinds of work' },
		...(autonomyQuery.data?.data.tasks ?? []).map((item) => ({
			value: item.task,
			label: item.label
		})),
		{ value: 'assistant_plan', label: 'Assistant' },
		{ value: 'workflow', label: 'Automations' }
	]);
	const actorOptions = [
		{ value: 'all', label: 'Anyone' },
		{ value: 'ai', label: 'AI' },
		{ value: 'automation', label: 'Automations' },
		{ value: 'assistant', label: 'Assistant' }
	];

	const feedQuery = createInfiniteQuery(() => ({
		queryKey: ['ai-activity', projectId, task, actorType, showUndone] as const,
		initialPageParam: undefined as string | undefined,
		queryFn: ({ pageParam, signal }) =>
			listAiActivity(
				projectId,
				{
					cursor: pageParam,
					limit: 30,
					task: task === 'all' ? undefined : task,
					actor_type: actorType === 'all' ? undefined : actorType,
					undone: showUndone ? undefined : false
				},
				{ signal }
			),
		getNextPageParam: (lastPage) => lastPage.data.next_cursor ?? undefined
	}));

	const entries = $derived(feedQuery.data?.pages.flatMap((page) => page.data.items) ?? []);
	const groups = $derived(groupActivity(entries));
	const overview = $derived(overviewQuery.data?.data);

	function labelFor(options: { value: string; label: string }[], value: string): string {
		return options.find((option) => option.value === value)?.label ?? value;
	}

	async function refresh(): Promise<void> {
		await queryClient.invalidateQueries({ queryKey: activityKeys.all(projectId) });
		await overviewQuery.refetch();
		// Undoing can change what other screens show.
		await queryClient.invalidateQueries({ queryKey: activityKeys.screeningQueue(projectId) });
	}

	async function undo(entry: AiActivityDto): Promise<void> {
		if (busyId) return;
		busyId = entry.id;
		try {
			await undoMutation.mutateAsync({ projectId, activityId: entry.id });
			notifySuccess('Undone', 'The change was reversed and the undo was recorded.');
			await refresh();
		} catch (error) {
			notifyError('Could not undo this', error, 'This action could not be undone.');
			await refresh();
		} finally {
			busyId = null;
		}
	}

	async function undoBatch(batchId: string): Promise<void> {
		if (busyId) return;
		busyId = batchId;
		confirmingBatch = null;
		try {
			const response = await undoBatchMutation.mutateAsync({ projectId, batchId });
			const { undone, failed } = response.data;
			if (failed.length > 0) {
				notifyInfo(
					`Undid ${undone}, left ${failed.length} as they are`,
					failed[0]?.message ?? 'Some changes were edited since and were not touched.'
				);
			} else {
				notifySuccess(`Undid ${undone} ${undone === 1 ? 'change' : 'changes'}`);
			}
			await refresh();
		} catch (error) {
			notifyError('Could not undo this group', error, 'The group could not be undone.');
			await refresh();
		} finally {
			busyId = null;
		}
	}
</script>

<PageTemplate testId="ai-activity-page" maxWidth="narrow">
	<header class="flex flex-col gap-1">
		<h2 class="editorial-title text-xl">AI activity</h2>
		<p class="max-w-2xl text-sm text-muted-foreground">
			Everything the AI, your automations and the assistant did in this project. Each action
			can be undone.
		</p>
	</header>

	{#if overview}
		<div class="flex flex-wrap gap-x-6 gap-y-2 text-sm" data-testid="ai-activity-overview">
			<a
				class="hover:text-primary"
				href={resolve('/projects/[projectId]/extraction', { projectId })}
			>
				<span class="font-semibold tabular-nums">{overview.to_verify}</span> to verify
			</a>
			<a
				class="hover:text-primary"
				href={resolve(
					`/projects/${encodeURIComponent(projectId)}/screening/title-abstract?view=conflicts`
				)}
			>
				<span class="font-semibold tabular-nums">{overview.open_conflicts}</span>
				{overview.open_conflicts === 1 ? 'conflict' : 'conflicts'} to settle
			</a>
			<span>
				<span class="font-semibold tabular-nums">{overview.undoable}</span> can still be undone
			</span>
		</div>
	{/if}

	<div class="flex flex-wrap items-center gap-3" role="group" aria-label="Filter activity">
		<Select.Root type="single" bind:value={task}>
			<Select.Trigger aria-label="Kind of work" class="w-52"
				>{labelFor(taskOptions, task)}</Select.Trigger
			>
			<Select.Content>
				<Select.Group>
					{#each taskOptions as option (option.value)}
						<Select.Item value={option.value} label={option.label} />
					{/each}
				</Select.Group>
			</Select.Content>
		</Select.Root>
		<Select.Root type="single" bind:value={actorType}>
			<Select.Trigger aria-label="Who did it" class="w-40"
				>{labelFor(actorOptions, actorType)}</Select.Trigger
			>
			<Select.Content>
				<Select.Group>
					{#each actorOptions as option (option.value)}
						<Select.Item value={option.value} label={option.label} />
					{/each}
				</Select.Group>
			</Select.Content>
		</Select.Root>
		<label class="flex items-center gap-2 text-sm">
			<Switch bind:checked={showUndone} aria-label="Show undone actions" />
			Show undone
		</label>
	</div>

	{#if feedQuery.isPending}
		<div class="flex flex-col gap-3" aria-label="Loading activity">
			{#each { length: 4 }, index (index)}<Skeleton class="h-16 w-full" />{/each}
		</div>
	{:else if feedQuery.error}
		<StatePanel
			state="error"
			title="Activity unavailable"
			description={feedQuery.error.message}
		/>
	{:else if groups.length === 0}
		<StatePanel
			state="empty"
			title="Nothing yet"
			description="When the AI, an automation or the assistant changes something, it appears here."
		/>
	{:else}
		<ol class="flex flex-col" data-testid="ai-activity-list">
			{#each groups as group (group.key)}
				{#if group.kind === 'single'}
					<li class="border-b py-3">{@render item(group.entry)}</li>
				{:else}
					{@const remaining = undoableCount(group.entries)}
					<li class="border-b py-3" data-testid="ai-activity-batch">
						<div class="mb-2 flex flex-wrap items-center justify-between gap-2">
							<p class="text-xs font-medium text-muted-foreground">
								{group.entries.length} actions from the same run
							</p>
							{#if remaining > 0}
								{#if confirmingBatch === group.batchId}
									<span class="flex items-center gap-2">
										<Button
											size="xs"
											variant="destructive"
											disabled={busyId !== null}
											onclick={() => void undoBatch(group.batchId)}
											>Undo {remaining}
											{remaining === 1 ? 'change' : 'changes'}</Button
										>
										<Button
											size="xs"
											variant="ghost"
											onclick={() => (confirmingBatch = null)}>Keep</Button
										>
									</span>
								{:else}
									<Button
										size="xs"
										variant="outline"
										disabled={busyId !== null}
										onclick={() => (confirmingBatch = group.batchId)}
										><Undo2 data-icon="inline-start" />Undo all</Button
									>
								{/if}
							{/if}
						</div>
						<ul class="flex flex-col gap-3 border-l pl-4">
							{#each group.entries as entry (entry.id)}
								<li>{@render item(entry)}</li>
							{/each}
						</ul>
					</li>
				{/if}
			{/each}
		</ol>
		{#if feedQuery.hasNextPage}
			<Button
				variant="outline"
				class="self-center"
				disabled={feedQuery.isFetchingNextPage}
				onclick={() => void feedQuery.fetchNextPage()}>Show older activity</Button
			>
		{/if}
	{/if}
</PageTemplate>

{#snippet item(entry: AiActivityDto)}
	<div class="flex flex-col gap-1.5" data-testid="ai-activity-item">
		<div class="flex items-start justify-between gap-3">
			<div class="flex min-w-0 items-start gap-2">
				{#if entry.actor_type === 'ai'}<Sparkles
						class="mt-0.5 size-4 shrink-0 text-muted-foreground"
						aria-hidden="true"
					/>{:else if entry.actor_type === 'assistant'}<Bot
						class="mt-0.5 size-4 shrink-0 text-muted-foreground"
						aria-hidden="true"
					/>{:else}<Workflow
						class="mt-0.5 size-4 shrink-0 text-muted-foreground"
						aria-hidden="true"
					/>{/if}
				<div class="min-w-0">
					<p
						class={entry.undone_at
							? 'text-sm text-muted-foreground line-through'
							: 'text-sm'}
					>
						{entry.summary}
					</p>
					<p
						class="mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground"
					>
						<span>{ACTOR_LABEL[entry.actor_type] ?? entry.actor_type}</span>
						<span>·</span>
						<span>{entry.task_label}</span>
						<span>·</span>
						<time
							datetime={entry.created_at}
							title={new Date(entry.created_at).toLocaleString()}
							>{relativeTime(entry.created_at)}</time
						>
						{#if entry.model}<Badge variant="outline" size="sm">{entry.model}</Badge
							>{/if}
						{#if entry.undone_at}<Badge variant="secondary" size="sm"
								>Undone{entry.undone_by ? ` by ${entry.undone_by}` : ''}</Badge
							>{/if}
					</p>
				</div>
			</div>
			{#if entry.undoable && !entry.undone_at}
				<Button
					size="xs"
					variant="ghost"
					disabled={busyId !== null}
					onclick={() => void undo(entry)}
					aria-label={`Undo: ${entry.summary}`}
					><Undo2 data-icon="inline-start" />Undo</Button
				>
			{/if}
		</div>
		{#if entry.evidence.length > 0 || entry.prompt_version}
			<details class="pl-6 text-xs text-muted-foreground">
				<summary class="cursor-pointer">Why and what it was based on</summary>
				<div class="flex flex-col gap-2 pt-1.5">
					{#each entry.evidence as quote (quote.label + quote.quote)}
						<blockquote class="border-l-2 pl-3">
							<span class="font-medium text-foreground">{quote.label}</span>
							{#if quote.quote}<p class="mt-0.5">“{quote.quote}”</p>{/if}
						</blockquote>
					{/each}
					{#if entry.prompt_version}<p>
							Prompt {entry.prompt_version}{entry.ai_run_id
								? ` · run ${entry.ai_run_id}`
								: ''}
						</p>{/if}
				</div>
			</details>
		{/if}
	</div>
{/snippet}
