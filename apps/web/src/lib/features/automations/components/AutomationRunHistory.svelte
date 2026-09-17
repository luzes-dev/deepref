<script lang="ts">
	import type { AutomationRunDto } from '$lib/api/generated/models';
	import { Button } from '@deepref/ui/button';
	import { Badge } from '@deepref/ui/badge';
	import { formatCostMicros, formatInteger, formatTimestamp, labelForStatus } from '../helpers';

	let {
		runs,
		selectedRun,
		loading,
		onSelect
	}: {
		runs: AutomationRunDto[];
		selectedRun?: AutomationRunDto;
		loading: boolean;
		onSelect: (id: string) => void;
	} = $props();
</script>

<details class="rounded-lg border border-border p-3" open={selectedRun !== undefined}>
	<summary class="cursor-pointer text-sm font-medium">Run history · {runs.length}</summary>
	<div class="mt-3 flex flex-col gap-3">
		{#if runs.length === 0}
			<p class="text-xs text-muted-foreground">No runs yet.</p>
		{/if}
		{#each runs as run (run.id)}
			<Button
				variant="ghost"
				class="h-auto justify-between gap-2 text-left whitespace-normal"
				onclick={() => onSelect(run.id)}
			>
				<span>{formatTimestamp(run.created_at)}</span>
				<Badge variant="outline">{labelForStatus(run.status)}</Badge>
			</Button>
		{/each}
		{#if loading}
			<p role="status" class="text-xs text-muted-foreground">Loading run details…</p>
		{:else if selectedRun}
			<div class="flex flex-col gap-3 text-xs" data-testid="automation-run-details">
				<p class="font-medium">{labelForStatus(selectedRun.status)}</p>
				{#if selectedRun.error}<p role="alert" class="text-destructive">
						{selectedRun.error}
					</p>{/if}
				<dl class="grid grid-cols-2 gap-2">
					<dt class="text-muted-foreground">Job status</dt>
					<dd>{labelForStatus(selectedRun.job.status)}</dd>
					<dt class="text-muted-foreground">Attempts</dt>
					<dd>{selectedRun.job.attempts} / {selectedRun.job.max_attempts}</dd>
					<dt class="text-muted-foreground">Input tokens</dt>
					<dd>{formatInteger(selectedRun.usage.input_tokens)}</dd>
					<dt class="text-muted-foreground">Output tokens</dt>
					<dd>{formatInteger(selectedRun.usage.output_tokens)}</dd>
					<dt class="text-muted-foreground">Cost</dt>
					<dd class="break-words">{formatCostMicros(selectedRun.usage.cost_micros)}</dd>
				</dl>
				{#if selectedRun.job.last_error}<p class="text-destructive">
						{selectedRun.job.last_error}
					</p>{/if}
				<ol class="flex flex-col gap-2">
					{#each selectedRun.steps as step (step.id)}
						<li class="flex flex-col gap-1 rounded-md bg-muted p-2">
							<span class="break-all">{step.ordinal + 1}. {step.key}</span>
							<span class="text-muted-foreground"
								>{labelForStatus(step.status)} · {step.attempts} attempts</span
							>
							{#if step.error}<span class="text-destructive">{step.error}</span>{/if}
						</li>
					{/each}
				</ol>
			</div>
		{/if}
	</div>
</details>
