<script lang="ts">
	import { resolve } from '$app/paths';
	import { untrack } from 'svelte';
	import type {
		AiAppraisalPrefillEvidenceDto,
		AiAppraisalPrefillProposalPayload,
		AppraisalDefinitionDto,
		CompleteAppraisalRequest,
		DocumentBlockDto,
		JudgmentSuggestionDto
	} from '$lib/api/generated/models';
	import { suggestAppraisalJudgments } from '$lib/api/generated/appraisal/appraisal';
	import {
		buildAppraisalPayload,
		createInitialFormState,
		definitionRequiresEvidence,
		judgmentIsComplete,
		questionHasRequiredEvidence,
		questionIsAsked,
		reconcileConditionalResponses,
		type AppraisalFormState
	} from '../form';
	import {
		appraisalEvidenceLabel,
		appraisalEvidenceTechnical,
		resolveAppraisalEvidence
	} from '../ai-prefill';
	import EvidenceLabel from '$lib/features/evidence/EvidenceLabel.svelte';
	import { fullTextUrlString } from '$lib/features/full-text/url';
	import { responseIsComplete } from '../renderer';
	import { blockSnippet } from '../evidence-search';
	import {
		driverLabels,
		judgmentLabel,
		OVERALL_TARGET,
		suggestionForDomain,
		targetsMissingReason,
		type JudgmentTarget
	} from '../suggestion';
	import EvidenceBlockPicker from './EvidenceBlockPicker.svelte';
	import JudgmentSuggestionPanel from './JudgmentSuggestionPanel.svelte';
	import * as Alert from '@deepref/ui/alert';
	import { Button } from '@deepref/ui/button';
	import { Spinner } from '@deepref/ui/spinner';
	import { CheckCircle2, Trash2 } from '@lucide/svelte';

	type Props = {
		definition: AppraisalDefinitionDto;
		blocks: DocumentBlockDto[];
		onSubmit: (request: CompleteAppraisalRequest, state: AppraisalFormState) => Promise<void>;
		initialState?: AppraisalFormState;
		/** Called with a snapshot whenever the answers change, so a parent can keep a draft. */
		onStateChange?: (state: AppraisalFormState) => void;
		projectId?: string;
		reportId?: string;
		originalPrefill?: AiAppraisalPrefillProposalPayload;
		submitLabel?: string;
	};

	let {
		definition,
		blocks,
		onSubmit,
		initialState,
		onStateChange,
		projectId = '',
		reportId = '',
		originalPrefill,
		submitLabel = 'Complete appraisal'
	}: Props = $props();

	function copyFormState(source: AppraisalFormState): AppraisalFormState {
		return {
			responses: { ...source.responses },
			evidence: Object.fromEntries(
				Object.entries(source.evidence).map(([questionId, selections]) => [
					questionId,
					selections.map((selection) => ({ ...selection }))
				])
			),
			domainJudgments: { ...source.domainJudgments },
			overallJudgment: source.overallJudgment,
			overrideReasons: { ...source.overrideReasons }
		};
	}

	let formState = $state<AppraisalFormState>(
		untrack(() => (initialState ? copyFormState(initialState) : createInitialFormState()))
	);
	$effect(() => {
		const snapshot = $state.snapshot(formState);
		untrack(() => onStateChange?.(snapshot));
	});
	let error = $state<string | undefined>();
	let submitting = $state(false);
	const blockById = $derived(new Map(blocks.map((block) => [block.id, block])));

	// The rule suggestion follows the answers. Requests are debounced, and only the latest
	// reply is kept, so a slow response never replaces a newer suggestion.
	let suggestion = $state<JudgmentSuggestionDto | undefined>();
	let suggestionFailed = $state(false);
	let suggestionRequest = 0;
	const suggestionStatus = $derived<'pending' | 'unavailable' | 'ready'>(
		suggestionFailed ? 'unavailable' : suggestion === undefined ? 'pending' : 'ready'
	);
	$effect(() => {
		const responses = { ...formState.responses };
		if (!projectId) return;
		const request = ++suggestionRequest;
		const timer = setTimeout(() => {
			void suggestAppraisalJudgments(projectId, definition.id, definition.version, {
				responses
			})
				.then((result) => {
					if (request !== suggestionRequest) return;
					suggestion = result.data;
					suggestionFailed = false;
				})
				.catch(() => {
					if (request !== suggestionRequest) return;
					suggestionFailed = true;
				});
		}, 200);
		return () => clearTimeout(timer);
	});

	// Judgments that differ from their suggestion must carry a reason before completion.
	const judgmentTargets = $derived<JudgmentTarget[]>([
		...definition.domains.map((domain) => ({
			key: domain.id,
			label: domain.label,
			suggested: suggestionForDomain(suggestion, domain.id)?.judgment,
			chosen: formState.domainJudgments[domain.id] || undefined
		})),
		{
			key: OVERALL_TARGET,
			label: 'Overall judgment',
			suggested: suggestion?.overall_judgment,
			chosen: formState.overallJudgment || undefined
		}
	]);

	function setOverrideReason(target: string, value: string): void {
		formState.overrideReasons = { ...formState.overrideReasons, [target]: value };
	}

	function applySuggestion(target: string, value: string | null | undefined): void {
		if (!value) return;
		customOpen = { ...customOpen, [target]: false };
		if (target === OVERALL_TARGET) {
			formState.overallJudgment = value;
		} else {
			formState.domainJudgments = { ...formState.domainJudgments, [target]: value };
		}
	}

	const OTHER = '__other__';
	// Custom judgments live behind an "Other…" option so the common path stays a single select.
	let customOpen = $state<Record<string, boolean>>({});

	function isCustomJudgment(
		key: string,
		options: { value: string }[],
		value: string | undefined
	): boolean {
		return (
			customOpen[key] === true ||
			(value !== undefined && value !== '' && !options.some((o) => o.value === value))
		);
	}

	function judgmentSelectValue(
		key: string,
		options: { value: string }[],
		value: string | undefined
	): string {
		return isCustomJudgment(key, options, value) ? OTHER : (value ?? '');
	}

	/** Applies a judgment select change; returns the judgment value to store. */
	function chooseJudgment(
		key: string,
		options: { value: string }[],
		current: string | undefined,
		selected: string
	): string {
		if (selected === OTHER) {
			customOpen = { ...customOpen, [key]: true };
			return options.some((o) => o.value === current) ? '' : (current ?? '');
		}
		customOpen = { ...customOpen, [key]: false };
		return selected;
	}

	const questions = $derived(definition.domains.flatMap((domain) => domain.questions));
	const hasRequiredEvidence = $derived(definitionRequiresEvidence(definition));

	function setResponse(questionId: string, value: unknown): void {
		formState.responses = reconcileConditionalResponses(definition, {
			...formState.responses,
			[questionId]: value
		});
	}

	function addEvidenceBlock(questionId: string, block: DocumentBlockDto): void {
		const current = formState.evidence[questionId] ?? [];
		if (current.some((selection) => selection.blockId === block.id)) return;
		formState.evidence = {
			...formState.evidence,
			[questionId]: [...current, { documentId: block.document_id, blockId: block.id }]
		};
	}

	function removeEvidence(questionId: string, index: number): void {
		const selections = [...(formState.evidence[questionId] ?? [])];
		selections.splice(index, 1);
		formState.evidence = { ...formState.evidence, [questionId]: selections };
	}

	function inputValue(event: Event): string {
		return event.currentTarget instanceof HTMLInputElement
			? event.currentTarget.value
			: event.currentTarget instanceof HTMLSelectElement
				? event.currentTarget.value
				: '';
	}

	function selectedValue(questionId: string): string {
		const value = formState.responses[questionId];
		return typeof value === 'string' ? value : '';
	}

	function textValue(questionId: string): string {
		const value = formState.responses[questionId];
		return typeof value === 'string' ? value : '';
	}

	function originalEvidence(questionId: string): AiAppraisalPrefillEvidenceDto[] {
		return (
			originalPrefill?.answers.find((answer) => answer.question_id === questionId)
				?.evidence ?? []
		);
	}

	async function submit(): Promise<void> {
		error = undefined;
		const incomplete = questions.find(
			(question) =>
				!responseIsComplete(question, formState.responses) ||
				!questionHasRequiredEvidence(question, formState.evidence)
		);
		if (incomplete) {
			error = `Complete the required response and evidence for “${incomplete.label}”.`;
			return;
		}
		if (!judgmentIsComplete(definition.overall_judgment, formState.overallJudgment)) {
			error = 'Select an overall judgment before completing the appraisal.';
			return;
		}
		const missingDomain = definition.domains.find(
			(domain) => !judgmentIsComplete(domain.judgment, formState.domainJudgments[domain.id])
		);
		if (missingDomain) {
			error = `Complete the judgment for “${missingDomain.label}”.`;
			return;
		}
		const missingReason = targetsMissingReason(judgmentTargets, formState.overrideReasons)[0];
		if (missingReason) {
			error = `Explain why the judgment for “${missingReason.label}” differs from the rule suggestion.`;
			return;
		}
		submitting = true;
		try {
			await onSubmit(buildAppraisalPayload(definition, formState), formState);
		} catch (submitError) {
			error =
				submitError instanceof Error
					? submitError.message
					: 'Appraisal could not be completed.';
		} finally {
			submitting = false;
		}
	}
</script>

<form
	class="flex min-w-0 flex-col gap-6"
	onsubmit={(event) => {
		event.preventDefault();
		void submit();
	}}
>
	<div class="sr-only">
		<h2>{definition.name} v{definition.version}</h2>
	</div>
	{#if definition.description}<p class="text-sm leading-6 text-muted-foreground">
			{definition.description}
		</p>{/if}
	{#if definition.applicability.note}<p class="text-xs leading-5 text-muted-foreground">
			{definition.applicability.note}
		</p>{/if}

	{#if error}
		<Alert.Root variant="destructive" role="alert">
			<Alert.Title>Appraisal needs attention</Alert.Title>
			<Alert.Description>{error}</Alert.Description>
		</Alert.Root>
	{/if}

	{#each definition.domains as domain (domain.id)}
		<fieldset class="flex min-w-0 flex-col gap-4 border-t py-5">
			<legend class="px-1 text-base font-semibold">{domain.label}</legend>
			{#if domain.description}<p class="text-sm leading-6 text-muted-foreground">
					{domain.description}
				</p>{/if}
			{#each domain.questions as question (question.id)}
				<div class="flex min-w-0 flex-col gap-2 border-b pb-4">
					<label for={question.id} class="text-sm font-medium">
						{question.label}{#if question.required}<span aria-hidden="true">
								*</span
							>{/if}
					</label>
					{#if question.help}<p
							class="text-xs text-muted-foreground"
							id={`${question.id}-help`}
						>
							{question.help}
						</p>{/if}
					{#if !questionIsAsked(question, formState.responses)}<p
							class="text-xs font-medium text-muted-foreground"
							data-testid={`not-asked-${question.id}`}
						>
							Not asked for these answers, so it is recorded as not applicable.
						</p>{/if}
					{#if question.answer_schema.kind === 'enum'}
						<select
							id={question.id}
							class="h-10 w-full rounded-lg border border-border/80 bg-background px-3 text-sm shadow-xs transition outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30"
							value={selectedValue(question.id)}
							disabled={!questionIsAsked(question, formState.responses)}
							onchange={(event) => setResponse(question.id, inputValue(event))}
							aria-describedby={question.help ? `${question.id}-help` : undefined}
						>
							<option value="">Select an answer</option>
							{#each question.answer_schema.options as option (option.value)}<option
									value={option.value}>{option.label}</option
								>{/each}
						</select>
					{:else if question.answer_schema.kind === 'boolean'}
						<label
							class="flex min-h-10 items-center gap-2 rounded-lg border border-border/80 bg-background px-3 text-sm shadow-xs"
						>
							<input
								id={question.id}
								type="checkbox"
								checked={formState.responses[question.id] === true}
								onchange={(event) =>
									setResponse(
										question.id,
										event.currentTarget instanceof HTMLInputElement &&
											event.currentTarget.checked
									)}
							/>
							Yes
						</label>
					{:else if question.answer_schema.kind === 'scale'}
						<div class="flex items-center gap-3">
							<input
								id={question.id}
								type="number"
								min={question.answer_schema.min}
								max={question.answer_schema.max}
								class="h-10 w-24 rounded-lg border border-border/80 bg-background px-3 text-sm shadow-xs transition outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30"
								value={typeof formState.responses[question.id] === 'number'
									? formState.responses[question.id]
									: ''}
								onchange={(event) =>
									setResponse(question.id, Number(inputValue(event)))}
							/>
							<span class="text-xs text-muted-foreground"
								>{question.answer_schema.min}–{question.answer_schema.max}</span
							>
						</div>
					{:else}
						<textarea
							id={question.id}
							class="min-h-24 w-full rounded-lg border border-border/80 bg-background p-3 text-sm shadow-xs transition outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30"
							maxlength={question.answer_schema.max_length}
							value={textValue(question.id)}
							oninput={(event) =>
								setResponse(
									question.id,
									event.currentTarget instanceof HTMLTextAreaElement
										? event.currentTarget.value
										: ''
								)}></textarea>
					{/if}
					{#if originalPrefill && projectId && reportId}
						<div
							class="flex flex-col gap-2 rounded-lg border border-primary/15 bg-primary/5 p-3"
							data-testid={`ai-evidence-${question.id}`}
						>
							<span class="text-xs font-medium text-muted-foreground"
								>Evidence provenance</span
							>
							{#each formState.evidence[question.id] ?? [] as selection, index (`${question.id}-source-${index}`)}
								{@const evidence = resolveAppraisalEvidence(
									selection,
									originalEvidence(question.id),
									blocks
								)}
								{#if evidence}
									<a
										class="block max-w-full min-w-0 text-xs text-primary underline underline-offset-2"
										href={resolve(
											`/projects/${encodeURIComponent(projectId)}/screening/full-text${fullTextUrlString(
												{
													filter: 'all',
													report: reportId,
													page: evidence.page,
													block: evidence.document_block_id
												}
											)}`
										)}
										data-testid={`ai-evidence-link-${question.id}-${index}`}
									>
										<EvidenceLabel
											label={appraisalEvidenceLabel(
												evidence,
												blocks.find(
													(block) =>
														block.id === evidence.document_block_id
												)?.text
											)}
											technical={appraisalEvidenceTechnical(evidence)}
										/>
									</a>
								{:else}
									<p class="text-xs text-destructive">
										This evidence block is no longer available.
									</p>
								{/if}
							{:else}
								<p class="text-xs text-muted-foreground">No evidence selected.</p>
							{/each}
						</div>
					{/if}
					<div
						class="flex min-w-0 flex-col gap-2 rounded-lg border border-border/60 bg-muted/10 p-3"
					>
						<span class="text-xs font-medium text-muted-foreground"
							>{question.requires_evidence
								? 'Required evidence blocks'
								: 'Supporting evidence (optional)'}</span
						>
						{#each formState.evidence[question.id] ?? [] as selection, index (`${question.id}-${index}`)}
							<div
								class="flex min-w-0 items-start justify-between gap-2 rounded-md border border-border/60 bg-background p-2 text-xs"
							>
								<span class="min-w-0 break-words">
									{#if blockById.get(selection.blockId)}
										<span class="font-medium text-muted-foreground"
											>p. {blockById.get(selection.blockId)
												?.page_number}</span
										>
										{blockSnippet(blockById.get(selection.blockId)?.text ?? '')}
									{:else}
										Evidence block no longer available
									{/if}
								</span>
								<Button
									type="button"
									variant="ghost"
									size="sm"
									class="shrink-0"
									aria-label={`Remove evidence block ${index + 1}`}
									onclick={() => removeEvidence(question.id, index)}
								>
									<Trash2 aria-hidden="true" data-icon="inline-start" />Remove
								</Button>
							</div>
						{:else}
							<p class="text-xs text-muted-foreground">No evidence block selected.</p>
						{/each}
						<EvidenceBlockPicker
							idPrefix={`${question.id}-evidence`}
							questionLabel={question.label}
							{blocks}
							excludeIds={(formState.evidence[question.id] ?? []).map(
								(selection) => selection.blockId
							)}
							onSelect={(block) => addEvidenceBlock(question.id, block)}
						/>
					</div>
				</div>
			{/each}
			<label for={`${domain.id}-judgment`} class="text-sm font-medium"
				>Domain judgment{#if domain.judgment.required}<span aria-hidden="true">
						*</span
					>{/if}</label
			>
			<select
				id={`${domain.id}-judgment`}
				class="h-10 w-full rounded-lg border border-border/80 bg-background px-3 text-sm shadow-xs transition outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30"
				value={judgmentSelectValue(
					domain.id,
					domain.judgment.options,
					formState.domainJudgments[domain.id]
				)}
				onchange={(event) => {
					formState.domainJudgments = {
						...formState.domainJudgments,
						[domain.id]: chooseJudgment(
							domain.id,
							domain.judgment.options,
							formState.domainJudgments[domain.id],
							inputValue(event)
						)
					};
				}}
			>
				<option value="">Select a judgment</option>
				{#each domain.judgment.options as option (option.value)}<option value={option.value}
						>{option.label}</option
					>{/each}
				{#if domain.judgment.allow_custom}<option value={OTHER}>Other…</option>{/if}
			</select>
			{#if domain.judgment.allow_custom && isCustomJudgment(domain.id, domain.judgment.options, formState.domainJudgments[domain.id])}
				<label for={`${domain.id}-custom-judgment`} class="text-xs text-muted-foreground"
					>Custom judgment</label
				>
				<input
					id={`${domain.id}-custom-judgment`}
					class="h-10 w-full rounded-lg border border-border/80 bg-background px-3 text-sm shadow-xs transition outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30"
					value={formState.domainJudgments[domain.id] ?? ''}
					oninput={(event) => {
						formState.domainJudgments = {
							...formState.domainJudgments,
							[domain.id]: inputValue(event)
						};
					}}
				/>
			{/if}
			{#if suggestion?.available !== false}
				<JudgmentSuggestionPanel
					idPrefix={domain.id}
					status={suggestionStatus}
					suggested={suggestionForDomain(suggestion, domain.id)?.judgment}
					suggestedLabel={judgmentLabel(
						domain.judgment,
						suggestionForDomain(suggestion, domain.id)?.judgment ?? undefined
					)}
					drivers={driverLabels(
						definition,
						suggestionForDomain(suggestion, domain.id)?.drivers ?? []
					)}
					chosen={formState.domainJudgments[domain.id] || undefined}
					chosenLabel={judgmentLabel(
						domain.judgment,
						formState.domainJudgments[domain.id]
					)}
					reason={formState.overrideReasons[domain.id] ?? ''}
					onApply={() =>
						applySuggestion(
							domain.id,
							suggestionForDomain(suggestion, domain.id)?.judgment
						)}
					onReasonChange={(value) => setOverrideReason(domain.id, value)}
				/>
			{/if}
		</fieldset>
	{/each}

	<fieldset
		class="flex min-w-0 flex-col gap-2 rounded-xl border border-primary/15 bg-muted/10 p-4 sm:p-5"
	>
		<legend class="px-1 text-base font-semibold">Overall judgment</legend>
		<label for="overall-judgment" class="text-sm font-medium"
			>Final judgment{#if definition.overall_judgment.required}<span aria-hidden="true">
					*</span
				>{/if}</label
		>
		<select
			id="overall-judgment"
			class="h-10 w-full rounded-lg border border-border/80 bg-background px-3 text-sm shadow-xs transition outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30"
			value={judgmentSelectValue(
				'overall',
				definition.overall_judgment.options,
				formState.overallJudgment
			)}
			onchange={(event) => {
				formState.overallJudgment = chooseJudgment(
					'overall',
					definition.overall_judgment.options,
					formState.overallJudgment,
					inputValue(event)
				);
			}}
		>
			<option value="">Select a judgment</option>
			{#each definition.overall_judgment.options as option (option.value)}<option
					value={option.value}>{option.label}</option
				>{/each}
			{#if definition.overall_judgment.allow_custom}<option value={OTHER}>Other…</option>{/if}
		</select>
		{#if definition.overall_judgment.allow_custom && isCustomJudgment('overall', definition.overall_judgment.options, formState.overallJudgment)}
			<label for="overall-custom-judgment" class="text-xs text-muted-foreground"
				>Custom judgment</label
			>
			<input
				id="overall-custom-judgment"
				class="h-10 w-full rounded-lg border border-border/80 bg-background px-3 text-sm shadow-xs transition outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30"
				value={formState.overallJudgment}
				oninput={(event) => {
					formState.overallJudgment = inputValue(event);
				}}
			/>
		{/if}
		{#if suggestion?.available !== false}
			<JudgmentSuggestionPanel
				idPrefix={OVERALL_TARGET}
				status={suggestionStatus}
				suggested={suggestion?.overall_judgment}
				suggestedLabel={judgmentLabel(
					definition.overall_judgment,
					suggestion?.overall_judgment ?? undefined
				)}
				drivers={[]}
				notes={suggestion?.reviewer_notes ?? []}
				chosen={formState.overallJudgment || undefined}
				chosenLabel={judgmentLabel(definition.overall_judgment, formState.overallJudgment)}
				reason={formState.overrideReasons[OVERALL_TARGET] ?? ''}
				onApply={() => applySuggestion(OVERALL_TARGET, suggestion?.overall_judgment)}
				onReasonChange={(value) => setOverrideReason(OVERALL_TARGET, value)}
			/>
		{/if}
	</fieldset>

	<Button
		type="submit"
		class="w-full sm:w-fit"
		disabled={submitting || (hasRequiredEvidence && blocks.length === 0)}
	>
		{#if submitting}<Spinner data-icon="inline-start" />{/if}
		<CheckCircle2 aria-hidden="true" data-icon="inline-start" />{submitLabel}
	</Button>
	{#if hasRequiredEvidence && blocks.length === 0}<p class="text-xs text-muted-foreground">
			A parsed document block is required before completing an appraisal.
		</p>{/if}
</form>
