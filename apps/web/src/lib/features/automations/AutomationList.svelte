<script lang="ts">
	import { SvelteMap } from 'svelte/reactivity';
	import { resolve } from '$app/paths';
	import { useQueryClient } from '@tanstack/svelte-query';
	import PlayIcon from '@lucide/svelte/icons/play';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import WorkflowIcon from '@lucide/svelte/icons/workflow';
	import {
		createListProjectWorkflowRuns,
		createListWorkflows,
		deleteWorkflow,
		disableWorkflow,
		enableWorkflow,
		getListProjectWorkflowRunsQueryKey,
		getListWorkflowsQueryKey,
		startWorkflowRun
	} from '#lib/api/generated/automations/automations.js';
	import type { WorkflowDto } from '#lib/api/generated/models/index.js';
	import { notifyError, notifySuccess } from '#lib/features/notifications/toast.js';
	import PageTemplate from '#lib/shell/PageTemplate.svelte';
	import * as Dialog from '@deepref/ui/dialog';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import { Spinner } from '@deepref/ui/spinner';
	import { Switch } from '@deepref/ui/switch';
	import { formatWhen, graphOf, isActiveRun, RUN_STATUS_LABEL } from './api';
	import { PUBLISH_FIRST } from './builder-view';
	import { describeTrigger } from './model';
	import TemplateGallery from './TemplateGallery.svelte';

	let { projectId }: { projectId: string } = $props();

	const client = useQueryClient();
	const workflows = createListWorkflows(
		() => projectId,
		() => ({ query: { refetchInterval: 5000 } })
	);
	const runs = createListProjectWorkflowRuns(
		() => projectId,
		() => ({ limit: 100 }),
		() => ({ query: { refetchInterval: 5000 } })
	);

	let galleryOpen = $state(false);
	let pendingDelete = $state<WorkflowDto | null>(null);
	let busyId = $state<string | null>(null);

	const stats = $derived.by(() => {
		const map = new SvelteMap<
			string,
			{
				count: number;
				last: (typeof runs.data extends infer T ? T : never) | null;
				status: string;
			}
		>();
		for (const run of runs.data?.data ?? []) {
			const known = map.get(run.workflow_id);
			if (!known) map.set(run.workflow_id, { count: 1, last: null, status: run.status });
			else known.count += 1;
		}
		return map;
	});

	function refresh() {
		void client.invalidateQueries({ queryKey: getListWorkflowsQueryKey(projectId) });
		void client.invalidateQueries({ queryKey: getListProjectWorkflowRunsQueryKey(projectId) });
	}

	async function toggle(workflow: WorkflowDto, on: boolean) {
		busyId = workflow.id;
		try {
			if (on) await enableWorkflow(projectId, workflow.id);
			else await disableWorkflow(projectId, workflow.id);
			refresh();
		} catch (error) {
			// Show the server's own reason (for example "Publish the automation first.").
			notifyError(on ? 'Could not turn it on' : 'Could not turn it off', error);
		} finally {
			busyId = null;
		}
	}

	async function runNow(workflow: WorkflowDto) {
		busyId = workflow.id;
		try {
			await startWorkflowRun(projectId, workflow.id, {});
			notifySuccess('Started', `“${workflow.name}” is running now.`);
			refresh();
		} catch (error) {
			notifyError('Could not start it', error);
		} finally {
			busyId = null;
		}
	}

	async function remove() {
		const target = pendingDelete;
		if (!target) return;
		busyId = target.id;
		try {
			await deleteWorkflow(projectId, target.id);
			pendingDelete = null;
			refresh();
		} catch (error) {
			notifyError('Could not delete it', error);
		} finally {
			busyId = null;
		}
	}

	function statusVariant(status: string) {
		if (status === 'completed') return 'success' as const;
		if (status === 'failed') return 'destructive' as const;
		if (isActiveRun(status)) return 'info' as const;
		return 'secondary' as const;
	}
</script>

<!-- The shell's top bar already names the page with the one level-1 heading, so this page adds no second title. -->
<PageTemplate maxWidth="wide" testId="automations-page">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<p class="max-w-2xl text-sm text-muted-foreground">
			Little helpers that do routine work for you — when something happens, or on a schedule.
		</p>
		<Button onclick={() => (galleryOpen = true)} data-testid="new-automation">
			<PlusIcon /> New automation
		</Button>
	</div>

	{#if workflows.isPending}
		<p class="flex items-center gap-2 text-sm text-muted-foreground"><Spinner /> Loading…</p>
	{:else if workflows.isError}
		<p class="text-sm text-destructive">Your automations could not be loaded.</p>
	{:else if (workflows.data?.data.length ?? 0) === 0}
		<div
			class="flex flex-col items-center gap-3 rounded-lg border border-dashed border-border p-10 text-center"
		>
			<WorkflowIcon class="size-8 text-muted-foreground" />
			<p class="text-sm font-medium">No automations yet</p>
			<p class="max-w-md text-sm text-muted-foreground">
				For example: get a notification when a record is included, or look for new
				publications every Monday. Start from a ready-made idea or build your own.
			</p>
			<Button onclick={() => (galleryOpen = true)}><PlusIcon /> New automation</Button>
		</div>
	{:else}
		<ul
			class="flex flex-col divide-y divide-border rounded-lg border border-border bg-card"
			data-testid="automation-list"
		>
			{#each workflows.data?.data ?? [] as workflow (workflow.id)}
				{@const stat = stats.get(workflow.id)}
				{@const trigger = describeTrigger(graphOf(workflow))}
				<li
					class="flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3"
					data-testid="automation-row"
				>
					<div class="min-w-56 flex-1">
						<a
							class="text-sm font-semibold hover:underline focus-visible:outline-2 focus-visible:outline-ring"
							href={resolve('/projects/[projectId]/automations/[workflowId]', {
								projectId,
								workflowId: workflow.id
							})}>{workflow.name}</a
						>
						<p class="text-xs text-muted-foreground">{trigger}</p>
						{#if workflow.published_version == null}
							<p class="text-xs text-muted-foreground">Draft · not published yet</p>
						{:else if workflow.has_unpublished_changes}
							<p class="text-xs text-warning">Has changes that are not published</p>
						{/if}
					</div>
					<div class="flex w-40 flex-col gap-0.5 text-xs text-muted-foreground">
						{#if stat}
							<span class="flex items-center gap-1.5">
								<Badge size="sm" variant={statusVariant(stat.status)}
									>{RUN_STATUS_LABEL[stat.status] ?? stat.status}</Badge
								>
							</span>
							<span class="tabular-nums"
								>{stat.count >= 100 ? '100+' : stat.count}
								{stat.count === 1 ? 'run' : 'runs'}</span
							>
						{:else}
							<span>No runs yet</span>
						{/if}
						{#if workflow.last_run_at}<span>{formatWhen(workflow.last_run_at)}</span
							>{/if}
					</div>
					<div class="flex items-center gap-2">
						{#if workflow.trigger === 'manual' && workflow.published_version != null}
							<Button
								size="sm"
								variant="outline"
								disabled={busyId === workflow.id}
								onclick={() => runNow(workflow)}><PlayIcon /> Run now</Button
							>
						{/if}
						{#if workflow.trigger === 'manual'}
							<span class="text-sm text-muted-foreground" data-testid="manual-note">
								{workflow.published_version == null
									? 'Publish first to run it by hand'
									: 'Runs when you press Run'}
							</span>
						{:else}
							<label
								class="flex items-center gap-2 text-sm"
								title={workflow.published_version == null
									? PUBLISH_FIRST
									: undefined}
							>
								<Switch
									checked={workflow.status === 'enabled'}
									disabled={busyId === workflow.id ||
										workflow.published_version == null}
									onCheckedChange={(on) => toggle(workflow, on)}
									aria-label={`Turn “${workflow.name}” on`}
								/>
								{workflow.status === 'enabled' ? 'On' : 'Off'}
								{#if workflow.published_version == null}
									<span class="text-xs text-muted-foreground">Publish first</span>
								{/if}
							</label>
						{/if}
						<Button
							size="icon-sm"
							variant="ghost"
							aria-label={`Delete “${workflow.name}”`}
							onclick={() => (pendingDelete = workflow)}><Trash2Icon /></Button
						>
					</div>
				</li>
			{/each}
		</ul>
	{/if}
</PageTemplate>

<TemplateGallery {projectId} bind:open={galleryOpen} />

<Dialog.Root open={pendingDelete !== null} onOpenChange={(next) => !next && (pendingDelete = null)}>
	<Dialog.Content>
		<Dialog.Header>
			<Dialog.Title>Delete this automation?</Dialog.Title>
			<Dialog.Description>
				“{pendingDelete?.name}” and its history will be removed. This cannot be undone.
			</Dialog.Description>
		</Dialog.Header>
		<Dialog.Footer>
			<Button variant="outline" onclick={() => (pendingDelete = null)}>Keep it</Button>
			<Button variant="destructive" onclick={remove}>Delete</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
