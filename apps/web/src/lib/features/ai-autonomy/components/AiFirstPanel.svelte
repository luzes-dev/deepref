<script lang="ts">
	import { createMutation, useQueryClient } from '@tanstack/svelte-query';
	import {
		createGetAiFirstOverview,
		startAiFirstCohort,
		closeAiFirstCohort,
		drawAiFirstAudit,
		evaluateAiFirstAudit,
		finalizeAiFirstCohort,
		recoverAiFirstCohort
	} from '#lib/api/generated/ai/ai.js';
	import type { AiFirstForecastDto } from '#lib/api/generated/models/index.js';
	import { currentReviewerId, saveReviewerId } from '#lib/api/reviewer.js';
	import { notifyError } from '#lib/features/notifications/toast.js';
	import * as Alert from '@deepref/ui/alert';
	import * as Card from '@deepref/ui/card';
	import * as Field from '@deepref/ui/field';
	import * as ToggleGroup from '@deepref/ui/toggle-group';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import { Checkbox } from '@deepref/ui/checkbox';
	import { Input } from '@deepref/ui/input';
	import { Skeleton } from '@deepref/ui/skeleton';

	let { projectId }: { projectId: string } = $props();
	let target = $state<95 | 98>(95);
	let allowFinalization = $state(false);
	let acknowledgement = $state<string | null>(null);
	let reviewerDraft = $state(currentReviewerId());
	let reviewerError = $state('');
	let reviewerSaved = $state(false);
	let sampleDraft = $state('');
	let sampleError = $state('');
	const queryClient = useQueryClient();
	const overviewQuery = createGetAiFirstOverview(
		() => projectId,
		() => ({ query: { refetchInterval: 10000 } })
	);
	const overview = $derived(overviewQuery.data?.data);
	const activeCohort = $derived(
		overview?.cohorts.some((cohort) =>
			['open', 'closed', 'auditing', 'passed'].includes(cohort.status)
		)
	);

	type Action =
		| { kind: 'start'; target: 95 | 98; allowFinalization: boolean }
		| { kind: 'draw'; cohortId: string; sampleSize?: number }
		| { kind: 'close' | 'evaluate' | 'finalize' | 'recover'; cohortId: string };
	const action = createMutation(() => ({
		mutationFn: async (request: Action) => {
			switch (request.kind) {
				case 'start':
					return startAiFirstCohort(projectId, {
						target_percent: request.target,
						allow_finalization: request.allowFinalization
					});
				case 'close':
					return closeAiFirstCohort(projectId, request.cohortId);
				case 'draw':
					return drawAiFirstAudit(projectId, request.cohortId, {
						sample_size: request.sampleSize
					});
				case 'evaluate':
					return evaluateAiFirstAudit(projectId, request.cohortId);
				case 'finalize':
					return finalizeAiFirstCohort(projectId, request.cohortId, {
						acknowledge_reference_limits: true
					});
				case 'recover':
					return recoverAiFirstCohort(projectId, request.cohortId);
				default: {
					const exhaustive: never = request;
					return exhaustive;
				}
			}
		},
		onSuccess: async () => {
			acknowledgement = null;
			sampleDraft = '';
			sampleError = '';
			await queryClient.invalidateQueries();
		},
		onError: async (error) => {
			notifyError('AI-first action unavailable', error);
			await queryClient.invalidateQueries();
		}
	}));

	function selectedAuditForecast(
		forecast: AiFirstForecastDto,
		members: number,
		quarantined: number,
		draft: string
	) {
		const entered = Number(draft);
		const sampleSize =
			draft.trim() !== '' &&
			Number.isSafeInteger(entered) &&
			entered >= forecast.minimum_sample &&
			entered <= quarantined
				? entered
				: forecast.minimum_sample;
		const controls = Math.min(sampleSize, members - quarantined);
		const pass = members - quarantined + 2 * (sampleSize + controls);
		const savings = members - pass;
		return {
			...forecast,
			sample_size: sampleSize,
			controls,
			human_judgments_if_passed: pass,
			human_judgments_if_failed: members + sampleSize + 2 * controls,
			savings_vs_single: savings,
			recommended: savings >= 500 && savings * 5 >= members
		};
	}

	function drawAudit(cohortId: string, minimum: number, population: number): void {
		const sampleSize = sampleDraft.trim() === '' ? undefined : Number(sampleDraft);
		if (
			sampleSize !== undefined &&
			(!Number.isSafeInteger(sampleSize) || sampleSize < minimum || sampleSize > population)
		) {
			sampleError = `Choose a whole number from ${minimum} to ${population}, or leave blank for the baseline.`;
			return;
		}
		sampleError = '';
		action.mutate({ kind: 'draw', cohortId, sampleSize });
	}

	async function changeReviewer(): Promise<void> {
		try {
			saveReviewerId(reviewerDraft);
			reviewerDraft = currentReviewerId();
			reviewerError = '';
			reviewerSaved = true;
			await queryClient.cancelQueries();
			await queryClient.resetQueries();
		} catch (error) {
			reviewerSaved = false;
			reviewerError = error instanceof Error ? error.message : 'Could not save reviewer ID.';
		}
	}
</script>

<section class="flex flex-col gap-6" aria-labelledby="ai-first-title" data-testid="ai-first-panel">
	<div>
		<h2 id="ai-first-title" class="text-lg font-semibold">
			AI-first title &amp; abstract screening
		</h2>
		<p class="mt-1 text-sm text-muted-foreground">
			AI exclusions wait in quarantine without changing screening decisions. A closed cohort,
			blind human audit and separate approval are required before automated exclusions.
		</p>
	</div>
	<Alert.Root>
		<Alert.Title>Human reference retention, conditional on the reference labels</Alert.Title>
		<Alert.Description>
			The audit measures retention relative to human reference labels. Human reviewers can
			miss relevant records, so passing does not guarantee true recall or equivalence to dual
			human screening. Audit records appear in the ordinary queue without audit markers. Two
			fresh reviewers who have not seen AI opinions or earlier human decisions on this cohort
			must label each independently. After evaluation, either include or unsure retains a
			sampled quarantine record. A new positive on a previously excluded control ends this
			audit and returns the cohort to human review; other control labels preserve earlier
			decisions.
		</Alert.Description>
	</Alert.Root>
	<Field.FieldGroup>
		<Field.Field data-invalid={Boolean(reviewerError)}>
			<Field.FieldLabel for="ai-first-reviewer">Current reviewer ID</Field.FieldLabel>
			<div class="flex max-w-lg items-center gap-2">
				<Input
					id="ai-first-reviewer"
					bind:value={reviewerDraft}
					maxlength={128}
					aria-invalid={Boolean(reviewerError)}
					aria-describedby="ai-first-reviewer-description"
				/>
				<Button
					variant="outline"
					disabled={action.isPending}
					onclick={() => void changeReviewer()}>Use ID</Button
				>
			</div>
			<Field.FieldDescription id="ai-first-reviewer-description">
				Local, self-asserted identity for this browser tab; this is not authentication. Use
				your own consistent ID. Two independent people must use fresh, different IDs and
				must not have seen AI opinions or earlier human decisions on this cohort. Reusing an
				ID or changing your ID does not make you an independent reviewer.
			</Field.FieldDescription>
			{#if reviewerError}<Field.FieldError>{reviewerError}</Field.FieldError>
			{:else if reviewerSaved}<p class="text-xs text-muted-foreground" role="status">
					Reviewer ID saved; queues refreshed.
				</p>{/if}
		</Field.Field>
	</Field.FieldGroup>
	{#if overviewQuery.isPending}
		<Skeleton class="h-32 w-full" />
	{:else if overviewQuery.error}
		<Alert.Root variant="destructive">
			<Alert.Title>AI-first status unavailable</Alert.Title>
			<Alert.Description>{overviewQuery.error.message}</Alert.Description>
		</Alert.Root>
	{:else if overview}
		<div class="flex flex-wrap items-center gap-2">
			<Badge variant="outline">Owner ceiling: {overview.ceiling.replaceAll('_', ' ')}</Badge>
			{#if overview.suspended_reason}<Badge variant="warning">Suspended</Badge>{/if}
		</div>
		{#if overview.suspended_reason}
			<Alert.Root variant="warning">
				<Alert.Title>AI-first routing is suspended</Alert.Title>
				<Alert.Description
					>{overview.suspended_reason}. After returning the affected cohort to human
					screening, starting a new cohort records a fresh routing approval.</Alert.Description
				>
			</Alert.Root>
		{/if}
		<Card.Root>
			<Card.Header>
				<Card.Title>Start a cohort</Card.Title>
				<Card.Description
					>The current unscreened records form this cohort. Later imports need a new
					cohort. Each cohort has one fixed audit.</Card.Description
				>
			</Card.Header>
			<Card.Content>
				<Field.FieldGroup>
					<Field.Field>
						<Field.FieldLabel>Reference retention target</Field.FieldLabel>
						<ToggleGroup.Root
							type="single"
							variant="outline"
							size="sm"
							value={String(target)}
							aria-label="Reference retention target"
							onValueChange={(value) => {
								if (value === '95') target = 95;
								else if (value === '98') target = 98;
							}}
						>
							<ToggleGroup.Item value="95">95%</ToggleGroup.Item>
							<ToggleGroup.Item value="98">98%</ToggleGroup.Item>
						</ToggleGroup.Root>
					</Field.Field>
					<Field.Field orientation="horizontal">
						<Checkbox id="ai-first-permit" bind:checked={allowFinalization} />
						<Field.FieldContent>
							<Field.FieldLabel for="ai-first-permit"
								>Permit automated exclusion after this cohort passes</Field.FieldLabel
							>
							<Field.FieldDescription
								>Optional owner ceiling; passing never finalizes automatically.
								Leaving this off permits reversible routing only.</Field.FieldDescription
							>
						</Field.FieldContent>
					</Field.Field>
				</Field.FieldGroup>
			</Card.Content>
			<Card.Footer>
				<Button
					disabled={action.isPending || activeCohort}
					onclick={() => action.mutate({ kind: 'start', target, allowFinalization })}
					>Start reversible routing</Button
				>
			</Card.Footer>
		</Card.Root>
		{#each overview.cohorts as cohort (cohort.id)}
			<Card.Root>
				<Card.Header>
					<Card.Title>Cohort · {cohort.target_percent}% target</Card.Title>
					<Card.Description>{cohort.id}</Card.Description>
					<Card.Action
						><Badge
							variant={cohort.status === 'failed' || cohort.status === 'invalidated'
								? 'warning'
								: 'secondary'}>{cohort.status}</Badge
						></Card.Action
					>
				</Card.Header>
				<Card.Content class="flex flex-col gap-3">
					<p class="text-sm tabular-nums">
						Completed AI runs: {cohort.evaluated} / {cohort.members}
					</p>
					<p class="text-sm tabular-nums">
						{cohort.members} records · {cohort.quarantined} quarantined · {cohort.sampled}
						audit records ·
						{cohort.controls} interleaved controls ·
						{cohort.labels} / {(cohort.sampled + cohort.controls) * 2} independent labels
					</p>
					{#if cohort.status === 'open'}
						<p class="text-sm text-muted-foreground">
							Close admission after AI processing ends, including if the budget runs
							out. Finish all non-quarantined human records before drawing the audit.
							Unprocessed records remain in the human queue.
						</p>
					{:else if cohort.status === 'closed'}
						<p class="text-sm text-muted-foreground">
							Drawing freezes one random sample sized for a zero-miss baseline. It
							cannot be redrawn or extended after seeing labels. Complete the
							remaining human records first. Closed cohorts may have partial AI
							coverage; records without a completed AI run remain with humans.
						</p>
					{:else if cohort.status === 'auditing'}
						<p class="text-sm text-muted-foreground">
							Two fresh independent reviewers complete the interleaved ordinary
							screening queue. Neither may have seen earlier human decisions or AI
							opinions on this cohort. Evaluate once all labels are present; no
							repeated looks or favorable relabeling.
						</p>
					{/if}
					{#if cohort.status === 'closed' && cohort.forecast}
						{@const workload = selectedAuditForecast(
							cohort.forecast,
							cohort.members,
							cohort.quarantined,
							sampleDraft
						)}
						<Alert.Root variant={workload.recommended ? 'default' : 'warning'}>
							<Alert.Title
								>{workload.recommended
									? 'Estimated workload saving'
									: 'Savings below the default adoption threshold'}</Alert.Title
							>
							<Alert.Description>
								{cohort.forecast.human_records_remaining} non-quarantined human records
								remain. The zero-miss baseline needs {cohort.forecast
									.minimum_sample} audit records. The selected fixed sample is {workload.sample_size}
								audit records with {workload.controls} interleaved controls, two human
								labels per record. Estimated human judgments: {workload.human_judgments_if_passed}
								if passed, {workload.human_judgments_if_failed} if failed.
								{workload.savings_vs_single >= 0
									? `${workload.savings_vs_single} fewer`
									: `${-workload.savings_vs_single} more`}
								human judgments than single-human screening if passed. These conditional
								estimates omit administration and model costs. The sample size is frozen
								only when drawn.
							</Alert.Description>
						</Alert.Root>
						<Field.Field data-invalid={Boolean(sampleError)}>
							<Field.FieldLabel for={`ai-first-sample-${cohort.id}`}
								>Audit sample size (optional)</Field.FieldLabel
							>
							<Input
								id={`ai-first-sample-${cohort.id}`}
								type="number"
								min={cohort.forecast.minimum_sample}
								max={cohort.quarantined}
								step="1"
								value={sampleDraft}
								oninput={(event) => {
									sampleDraft = event.currentTarget.value;
								}}
								aria-invalid={Boolean(sampleError)}
							/>
							<Field.FieldDescription
								>Leave blank for the minimum zero-miss baseline. You may choose a
								larger fixed sample before seeing any labels.</Field.FieldDescription
							>
							{#if sampleError}<Field.FieldError>{sampleError}</Field.FieldError>{/if}
						</Field.Field>
					{/if}

					{#if cohort.invalidation_reason}<p class="text-sm text-destructive">
							{cohort.invalidation_reason}
						</p>{/if}
					{#if cohort.result}
						<p class="text-sm tabular-nums">
							Audit {cohort.result.passed ? 'passed' : 'failed'} · {cohort.result
								.observed_relevant} reference-relevant records found · p = {cohort.result.p_value.toPrecision(
								3
							)}
						</p>
						{#if cohort.result.reference_retention != null}
							<p class="text-sm tabular-nums">
								Estimated retention of the human reference: {(
									cohort.result.reference_retention * 100
								).toFixed(1)}%
							</p>
						{/if}
					{/if}
					{#if cohort.status === 'passed'}
						<Field.Field orientation="horizontal">
							<Checkbox
								id={`ai-first-ack-${cohort.id}`}
								checked={acknowledgement === cohort.id}
								onCheckedChange={(checked) => {
									acknowledgement = checked ? cohort.id : null;
								}}
							/>
							<Field.FieldLabel for={`ai-first-ack-${cohort.id}`}
								>I approve this cohort's automated exclusions and acknowledge that
								the human reference can be wrong; true recall is not guaranteed.</Field.FieldLabel
							>
						</Field.Field>
					{/if}
				</Card.Content>
				<Card.Footer class="flex flex-wrap gap-2">
					{#if cohort.status === 'open'}
						<Button
							disabled={action.isPending}
							onclick={() => action.mutate({ kind: 'close', cohortId: cohort.id })}
							>Close cohort</Button
						>
					{:else if cohort.status === 'closed'}
						<Button
							disabled={action.isPending}
							onclick={() =>
								drawAudit(
									cohort.id,
									cohort.forecast?.minimum_sample ?? 0,
									cohort.quarantined
								)}>Draw fixed audit</Button
						>
					{:else if cohort.status === 'auditing'}
						<Button
							disabled={action.isPending ||
								cohort.labels !== (cohort.sampled + cohort.controls) * 2}
							onclick={() => action.mutate({ kind: 'evaluate', cohortId: cohort.id })}
							>Evaluate once</Button
						>
					{:else if cohort.status === 'passed'}
						<Button
							disabled={action.isPending ||
								acknowledgement !== cohort.id ||
								overview.ceiling !== 'cohort_finalization'}
							onclick={() => action.mutate({ kind: 'finalize', cohortId: cohort.id })}
							>Approve automated exclusions</Button
						>
					{/if}
					{#if cohort.status !== 'declined'}
						<Button
							variant="outline"
							disabled={action.isPending}
							onclick={() => action.mutate({ kind: 'recover', cohortId: cohort.id })}
							>{cohort.status === 'finalized'
								? 'Recover and reopen exclusions'
								: 'Discard and return to human screening'}</Button
						>
					{/if}
				</Card.Footer>
			</Card.Root>
		{/each}
	{/if}
</section>
