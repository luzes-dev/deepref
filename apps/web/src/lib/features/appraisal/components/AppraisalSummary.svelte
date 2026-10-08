<script lang="ts">
	import type { AppraisalAssessmentDto, AppraisalDefinitionDto } from '$lib/api/generated/models';
	import { Button } from '@deepref/ui/button';
	import { CheckCircle2 } from '@lucide/svelte';
	import { responseLabel } from '../renderer';
	import { summarizeAssessment } from '../summary';

	type Props = {
		definition: AppraisalDefinitionDto;
		assessment: AppraisalAssessmentDto;
		onStartNew: () => void;
	};

	let { definition, assessment, onStartNew }: Props = $props();

	const summary = $derived(summarizeAssessment(definition, assessment, responseLabel));
</script>

<section
	class="flex min-w-0 flex-col gap-5 border-t pt-5"
	aria-label="Completed assessment"
	data-testid="appraisal-summary"
>
	<div class="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
		<p class="flex items-center gap-2 text-sm font-medium" role="status">
			<CheckCircle2 class="size-4 text-success" aria-hidden="true" />
			Completed · {new Date(assessment.completed_at).toLocaleString()}
		</p>
		<p class="text-xs text-muted-foreground tabular-nums">
			{summary.evidenceCount}
			{summary.evidenceCount === 1 ? 'evidence reference' : 'evidence references'}
		</p>
	</div>

	{#each summary.domains as domain (domain.id)}
		<div class="flex min-w-0 flex-col gap-2 border-b pb-4">
			<div class="flex flex-wrap items-baseline justify-between gap-2">
				<h3 class="text-sm font-semibold">{domain.label}</h3>
				<span class="text-sm">
					{domain.judgment ?? 'No judgment'}
					{#if domain.suggested}<span class="text-xs text-muted-foreground"
							>· rule suggested {domain.suggested}</span
						>{/if}
				</span>
			</div>
			<dl class="flex flex-col gap-1.5 text-sm">
				{#each domain.answers as answer (answer.id)}
					<div class="flex flex-wrap justify-between gap-x-4">
						<dt class="text-muted-foreground">{answer.label}</dt>
						<dd class="font-medium">{answer.value}</dd>
					</div>
				{/each}
			</dl>
		</div>
	{/each}

	<div class="flex flex-wrap items-baseline justify-between gap-2">
		<h3 class="text-sm font-semibold">Overall judgment</h3>
		<span class="text-sm">
			{summary.overall ?? 'No judgment'}
			{#if summary.overallSuggested}<span class="text-xs text-muted-foreground"
					>· rule suggested {summary.overallSuggested}</span
				>{/if}
		</span>
	</div>

	{#if summary.overrides.length > 0}
		<dl class="flex flex-col gap-2 text-sm" data-testid="appraisal-override-reasons">
			<dt class="font-semibold">Reasons for changing the rule suggestion</dt>
			{#each summary.overrides as override (override.label)}
				<dd class="flex flex-col gap-0.5 border-l-2 pl-3">
					<span class="font-medium">{override.label}</span>
					<span class="break-words text-muted-foreground">{override.reason}</span>
				</dd>
			{/each}
		</dl>
	{/if}

	<div class="flex flex-wrap items-center gap-3">
		<Button variant="outline" size="sm" onclick={onStartNew}>Start a new assessment</Button>
		<span class="text-xs text-muted-foreground"
			>Use the framework menu above to appraise with a different framework.</span
		>
	</div>
</section>
