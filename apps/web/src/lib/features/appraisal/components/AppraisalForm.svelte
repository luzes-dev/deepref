<script lang="ts">
	import { resolve } from '$app/paths';
	import { untrack } from 'svelte';
	import type {
		AiAppraisalPrefillEvidenceDto,
		AiAppraisalPrefillProposalPayload,
		AppraisalDefinitionDto,
		CompleteAppraisalRequest,
		DocumentBlockDto
	} from '$lib/api/generated/models';
	import {
		buildAppraisalPayload,
		createInitialFormState,
		definitionRequiresEvidence,
		judgmentIsComplete,
		questionHasRequiredEvidence,
		type AppraisalFormState
	} from '../form';
	import { appraisalEvidenceLabel, resolveAppraisalEvidence } from '../ai-prefill';
	import { fullTextUrlString } from '$lib/features/full-text/url';
	import { responseIsComplete } from '../renderer';
	import * as Alert from '$lib/components/ui/alert';
	import { Button } from '$lib/components/ui/button';
	import { Spinner } from '$lib/components/ui/spinner';
	import { CheckCircle2, Plus, Trash2 } from '@lucide/svelte';

	type Props = {
		definition: AppraisalDefinitionDto;
		blocks: DocumentBlockDto[];
		onSubmit: (request: CompleteAppraisalRequest, state: AppraisalFormState) => Promise<void>;
		initialState?: AppraisalFormState;
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
			overallJudgment: source.overallJudgment
		};
	}

	let formState = $state<AppraisalFormState>(
		untrack(() => (initialState ? copyFormState(initialState) : createInitialFormState()))
	);
	let error = $state<string | undefined>();
	let submitting = $state(false);

	const questions = $derived(definition.domains.flatMap((domain) => domain.questions));
	const hasRequiredEvidence = $derived(definitionRequiresEvidence(definition));

	function setResponse(questionId: string, value: unknown): void {
		formState.responses = { ...formState.responses, [questionId]: value };
	}

	function addEvidence(questionId: string): void {
		formState.evidence = {
			...formState.evidence,
			[questionId]: [
				...(formState.evidence[questionId] ?? []),
				{ documentId: '', blockId: '' }
			]
		};
	}

	function setEvidence(questionId: string, index: number, value: string): void {
		const block = blocks.find((candidate) => candidate.id === value);
		const selections = [...(formState.evidence[questionId] ?? [])];
		if (!block) {
			selections.splice(index, 1);
			formState.evidence = { ...formState.evidence, [questionId]: selections };
			return;
		}
		selections[index] = { documentId: block.document_id, blockId: block.id };
		formState.evidence = {
			...formState.evidence,
			[questionId]: selections
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
	<div class="flex flex-col gap-2">
		<div class="flex flex-wrap items-center gap-2">
			<span class="text-xs font-semibold tracking-[0.12em] text-primary uppercase"
				>Versioned schema</span
			>
			<span class="text-xs text-muted-foreground">· v{definition.version}</span>
		</div>
		<h2 class="text-xl font-semibold tracking-tight">
			{definition.name} v{definition.version}
		</h2>
		<p class="text-sm leading-6 text-muted-foreground">{definition.description}</p>
	</div>

	{#if error}
		<Alert.Root variant="destructive" role="alert">
			<Alert.Title>Appraisal needs attention</Alert.Title>
			<Alert.Description>{error}</Alert.Description>
		</Alert.Root>
	{/if}

	{#each definition.domains as domain (domain.id)}
		<fieldset
			class="flex min-w-0 flex-col gap-4 rounded-xl border border-primary/15 bg-muted/10 p-4 sm:p-5"
		>
			<legend class="px-1 text-base font-semibold">{domain.label}</legend>
			{#if domain.description}<p class="text-sm leading-6 text-muted-foreground">
					{domain.description}
				</p>{/if}
			{#each domain.questions as question (question.id)}
				<div
					class="flex min-w-0 flex-col gap-2 rounded-lg border border-border/60 bg-background/70 p-3 sm:p-4"
				>
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
					{#if question.answer_schema.kind === 'enum'}
						<select
							id={question.id}
							class="h-10 w-full rounded-lg border border-border/80 bg-background px-3 text-sm shadow-xs transition outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30"
							value={selectedValue(question.id)}
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
										class="text-xs text-primary underline underline-offset-2"
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
										{appraisalEvidenceLabel(evidence)}
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
					{#if question.requires_evidence}
						<div
							class="flex flex-col gap-2 rounded-lg border border-border/60 bg-muted/10 p-3"
						>
							<span class="text-xs font-medium text-muted-foreground"
								>Required evidence blocks</span
							>
							{#each formState.evidence[question.id] ?? [] as selection, index (`${question.id}-${index}`)}
								<div
									class="flex min-w-0 flex-col gap-2 sm:flex-row sm:items-center sm:gap-2"
								>
									<label for={`${question.id}-evidence-${index}`} class="sr-only"
										>Select evidence block {index + 1}</label
									>
									<select
										id={`${question.id}-evidence-${index}`}
										class="h-10 min-w-0 flex-1 rounded-lg border border-border/80 bg-background px-3 text-sm shadow-xs transition outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30"
										value={selection.blockId}
										onchange={(event) =>
											setEvidence(question.id, index, inputValue(event))}
									>
										<option value="">Select a document block</option>
										{#each blocks as block (block.id)}<option value={block.id}
												>p. {block.page_number} · {block.text.slice(
													0,
													100
												)}</option
											>{/each}
									</select>
									<Button
										type="button"
										variant="ghost"
										size="sm"
										class="self-start text-muted-foreground sm:self-auto"
										onclick={() => removeEvidence(question.id, index)}
									>
										<Trash2 aria-hidden="true" data-icon="inline-start" />Remove
									</Button>
								</div>
							{/each}
							<Button
								type="button"
								variant="outline"
								size="sm"
								class="self-start"
								onclick={() => addEvidence(question.id)}
							>
								<Plus aria-hidden="true" data-icon="inline-start" />Add evidence
								block
							</Button>
						</div>
					{/if}
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
				value={domain.judgment.options.some(
					(option) => option.value === formState.domainJudgments[domain.id]
				)
					? (formState.domainJudgments[domain.id] ?? '')
					: ''}
				onchange={(event) => {
					const value = inputValue(event);
					formState.domainJudgments = {
						...formState.domainJudgments,
						[domain.id]: value
					};
				}}
			>
				<option value="">Select a judgment</option>
				{#each domain.judgment.options as option (option.value)}<option value={option.value}
						>{option.label}</option
					>{/each}
			</select>
			{#if domain.judgment.allow_custom}
				<label for={`${domain.id}-custom-judgment`} class="text-xs text-muted-foreground"
					>Custom judgment</label
				>
				<input
					id={`${domain.id}-custom-judgment`}
					class="h-10 w-full rounded-lg border border-border/80 bg-background px-3 text-sm shadow-xs transition outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30"
					value={domain.judgment.options.some(
						(option) => option.value === formState.domainJudgments[domain.id]
					)
						? ''
						: (formState.domainJudgments[domain.id] ?? '')}
					oninput={(event) => {
						const value =
							event.currentTarget instanceof HTMLInputElement
								? event.currentTarget.value
								: '';
						formState.domainJudgments = {
							...formState.domainJudgments,
							[domain.id]: value
						};
					}}
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
			value={definition.overall_judgment.options.some(
				(option) => option.value === formState.overallJudgment
			)
				? formState.overallJudgment
				: ''}
			onchange={(event) => {
				formState.overallJudgment = inputValue(event);
			}}
		>
			<option value="">Select a judgment</option>
			{#each definition.overall_judgment.options as option (option.value)}<option
					value={option.value}>{option.label}</option
				>{/each}
		</select>
		{#if definition.overall_judgment.allow_custom}
			<label for="overall-custom-judgment" class="text-xs text-muted-foreground"
				>Custom judgment</label
			>
			<input
				id="overall-custom-judgment"
				class="h-10 w-full rounded-lg border border-border/80 bg-background px-3 text-sm shadow-xs transition outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30"
				value={definition.overall_judgment.options.some(
					(option) => option.value === formState.overallJudgment
				)
					? ''
					: formState.overallJudgment}
				oninput={(event) => {
					formState.overallJudgment =
						event.currentTarget instanceof HTMLInputElement
							? event.currentTarget.value
							: '';
				}}
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
