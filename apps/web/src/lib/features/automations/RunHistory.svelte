<script lang="ts">
	import { createListWorkflowRuns } from '$lib/api/generated/automations/automations';
	import type { WorkflowRunDto } from '$lib/api/generated/models';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import { Spinner } from '@deepref/ui/spinner';
	import { formatWhen, isActiveRun, RUN_STATUS_LABEL } from './api';
	import { reviewCountsLine, runLabel } from './runs';

	let {
		projectId,
		workflowId,
		onopen
	}: { projectId: string; workflowId: string; onopen: (run: WorkflowRunDto) => void } = $props();

	const runs = createListWorkflowRuns(
		() => projectId,
		() => workflowId,
		() => ({}),
		() => ({
			query: {
				refetchInterval: (query) =>
					query.state.data?.data.some((run) => isActiveRun(run.status)) ? 2000 : false
			}
		})
	);

	function variant(status: string) {
		if (status === 'completed') return 'success' as const;
		if (status === 'failed') return 'destructive' as const;
		if (status === 'running' || status === 'queued') return 'info' as const;
		return 'secondary' as const;
	}
</script>

<section class="mx-auto flex max-w-3xl flex-col gap-3" data-testid="run-history">
	<h2 class="text-base font-semibold">Run history</h2>
	{#if runs.isPending}
		<p class="flex items-center gap-2 text-sm text-muted-foreground"><Spinner /> Loading…</p>
	{:else if runs.isError}
		<p class="text-sm text-destructive">The history could not be loaded.</p>
	{:else if (runs.data?.data.length ?? 0) === 0}
		<p
			class="rounded-lg border border-dashed border-border p-6 text-center text-sm text-muted-foreground"
		>
			Nothing has run yet. Use “Test” on the canvas, or wait for the trigger.
		</p>
	{:else}
		<ul class="flex flex-col divide-y divide-border rounded-lg border border-border bg-card">
			{#each runs.data?.data ?? [] as run (run.id)}
				<li class="flex flex-wrap items-center gap-3 px-3 py-2.5">
					<Badge variant={variant(run.status)}
						>{RUN_STATUS_LABEL[run.status] ?? run.status}</Badge
					>
					<div class="min-w-0 flex-1">
						<p class="text-sm">{runLabel(run)}</p>
						{#if reviewCountsLine(run.review_counts)}
							<p class="text-xs tabular-nums" data-testid="run-review-counts">
								{reviewCountsLine(run.review_counts)}
							</p>
						{/if}
						<p class="text-xs text-muted-foreground">
							{formatWhen(run.created_at)}{run.test_mode
								? ' · made-up sample data'
								: ''}{run.error ? ` · ${run.error}` : ''}
						</p>
					</div>
					<Button size="sm" variant="outline" onclick={() => onopen(run)}
						>View on canvas</Button
					>
				</li>
			{/each}
		</ul>
	{/if}
</section>
