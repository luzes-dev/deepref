<script lang="ts">
	import type {
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
	import { responseIsComplete } from '../renderer';

	type Props = {
		definition: AppraisalDefinitionDto;
		blocks: DocumentBlockDto[];
		onSubmit: (request: CompleteAppraisalRequest) => Promise<void>;
	};

	let { definition, blocks, onSubmit }: Props = $props();
	let formState = $state<AppraisalFormState>(createInitialFormState());
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
			await onSubmit(buildAppraisalPayload(definition, formState));
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
	class="flex flex-col gap-6"
	onsubmit={(event) => {
		event.preventDefault();
		void submit();
	}}
>
	<div>
		<h2 class="text-xl font-semibold">{definition.name} v{definition.version}</h2>
		<p class="mt-1 text-sm text-muted-foreground">{definition.description}</p>
	</div>

	{#if error}
		<p
			class="rounded-md border border-destructive/50 bg-destructive/10 p-3 text-sm text-destructive"
			role="alert"
		>
			{error}
		</p>
	{/if}

	{#each definition.domains as domain (domain.id)}
		<fieldset class="flex flex-col gap-4 rounded-lg border p-4">
			<legend class="px-1 text-base font-semibold">{domain.label}</legend>
			{#if domain.description}<p class="text-sm text-muted-foreground">
					{domain.description}
				</p>{/if}
			{#each domain.questions as question (question.id)}
				<div class="flex flex-col gap-2">
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
							class="h-9 rounded-md border bg-transparent px-3 text-sm"
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
						<label class="flex items-center gap-2 text-sm">
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
								class="h-9 w-24 rounded-md border bg-transparent px-3 text-sm"
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
							class="min-h-24 rounded-md border bg-transparent p-3 text-sm"
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
					{#if question.requires_evidence}
						<div class="flex flex-col gap-2">
							<span class="text-xs font-medium text-muted-foreground"
								>Required evidence blocks</span
							>
							{#each formState.evidence[question.id] ?? [] as selection, index (`${question.id}-${index}`)}
								<div class="flex items-center gap-2">
									<label for={`${question.id}-evidence-${index}`} class="sr-only"
										>Select evidence block {index + 1}</label
									>
									<select
										id={`${question.id}-evidence-${index}`}
										class="h-9 min-w-0 flex-1 rounded-md border bg-transparent px-3 text-sm"
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
									<button
										type="button"
										class="text-xs text-muted-foreground underline"
										onclick={() => removeEvidence(question.id, index)}
										>Remove</button
									>
								</div>
							{/each}
							<button
								type="button"
								class="self-start text-xs font-medium text-primary underline"
								onclick={() => addEvidence(question.id)}>Add evidence block</button
							>
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
				class="h-9 rounded-md border bg-transparent px-3 text-sm"
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
					class="h-9 rounded-md border bg-transparent px-3 text-sm"
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

	<fieldset class="flex flex-col gap-2 rounded-lg border p-4">
		<legend class="px-1 text-base font-semibold">Overall judgment</legend>
		<label for="overall-judgment" class="text-sm font-medium"
			>Final judgment{#if definition.overall_judgment.required}<span aria-hidden="true">
					*</span
				>{/if}</label
		>
		<select
			id="overall-judgment"
			class="h-9 rounded-md border bg-transparent px-3 text-sm"
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
				class="h-9 rounded-md border bg-transparent px-3 text-sm"
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

	<button
		type="submit"
		class="inline-flex min-h-10 items-center justify-center rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90 disabled:pointer-events-none disabled:opacity-50"
		disabled={submitting || (hasRequiredEvidence && blocks.length === 0)}
		>Complete appraisal</button
	>
	{#if hasRequiredEvidence && blocks.length === 0}<p class="text-xs text-muted-foreground">
			A parsed document block is required before completing an appraisal.
		</p>{/if}
</form>
