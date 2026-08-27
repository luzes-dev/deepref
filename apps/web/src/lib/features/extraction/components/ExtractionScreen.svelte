<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import {
		createDecideAiProposal,
		createGenerateDataExtractionSuggestion,
		createListAiProposals
	} from '$lib/api/generated/ai/ai';
	import {
		createCreateExtractionField,
		createListExtractionFields,
		createListStudyExtractionValues,
		getListExtractionFieldsQueryKey,
		getListStudyExtractionValuesQueryKey
	} from '$lib/api/generated/extraction/extraction';
	import { ApiError } from '$lib/api/custom-fetch';
	import { createListProjectStudies } from '$lib/api/generated/studies/studies';
	import type {
		AiExtractedFieldDto,
		AiProposalDto,
		AiReviewedProposalPayload,
		CreateExtractionFieldRequest,
		ExtractionValueDto
	} from '$lib/api/generated/models';
	import { useQueryClient } from '@tanstack/svelte-query';
	import * as Alert from '$lib/components/ui/alert';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import { Checkbox } from '$lib/components/ui/checkbox';
	import * as Empty from '$lib/components/ui/empty';
	import * as Field from '$lib/components/ui/field';
	import { Input } from '$lib/components/ui/input';
	import { Separator } from '$lib/components/ui/separator';
	import * as Select from '$lib/components/ui/select';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { Spinner } from '$lib/components/ui/spinner';
	import { Textarea } from '$lib/components/ui/textarea';
	import { Brain, Check, FileSearch, Plus, RefreshCw, X } from '@lucide/svelte';
	import {
		buildExtractionEvidenceSearch,
		draftFromAiField,
		EXTRACTION_VALUE_TYPES,
		isExtractionValueType,
		serializeExtractionDrafts,
		validateExtractionDrafts,
		type ExtractionDraftField,
		type ExtractionDraftTypedValue,
		type ExtractionValueType
	} from '../helpers';
	import { parseExtractionLocation, updateExtractionLocation } from '../url';

	type DataExtractionProposal = AiProposalDto & {
		payload: Extract<AiProposalDto['payload'], { kind: 'data_extraction' }>;
	};
	type ActionStatus = 'provider-unavailable' | 'conflict' | 'validation' | 'error';

	let { projectId }: { projectId: string } = $props();
	const queryClient = useQueryClient();
	const location = $derived(parseExtractionLocation(page.url.searchParams));
	const selectedStudyId = $derived(location.studyId);

	const studiesQuery = createListProjectStudies(
		() => projectId,
		() => ({ limit: 100 })
	);
	const fieldsQuery = createListExtractionFields(() => projectId);
	const valuesQuery = createListStudyExtractionValues(
		() => projectId,
		() => selectedStudyId ?? '',
		() => ({ query: { enabled: Boolean(selectedStudyId) } })
	);
	const proposalsQuery = createListAiProposals(
		() => projectId,
		() => ({
			status: 'pending',
			task_kind: 'data_extraction',
			target_study_id: selectedStudyId,
			limit: 100
		}),
		() => ({ query: { enabled: Boolean(selectedStudyId) } })
	);
	const createFieldMutation = createCreateExtractionField();
	const generateMutation = createGenerateDataExtractionSuggestion();
	const decideMutation = createDecideAiProposal();

	const studies = $derived(studiesQuery.data?.data.items ?? []);
	const selectedStudy = $derived(studies.find((study) => study.id === selectedStudyId));
	const fields = $derived(fieldsQuery.data?.data ?? []);
	const values = $derived(valuesQuery.data?.data ?? []);
	const dataExtractionProposals = $derived(
		(proposalsQuery.data?.data.items ?? []).filter((proposal) =>
			isDataExtractionProposal(proposal, selectedStudyId)
		)
	);
	const activeProposal = $derived(dataExtractionProposals[0]);
	const selectedStudyLabel = $derived(
		selectedStudy?.title ?? selectedStudyId ?? 'Select a study'
	);
	const queryError = $derived(
		studiesQuery.error?.message ??
			fieldsQuery.error?.message ??
			valuesQuery.error?.message ??
			proposalsQuery.error?.message ??
			''
	);
	const loading = $derived(
		studiesQuery.isPending ||
			fieldsQuery.isPending ||
			(Boolean(selectedStudyId) && (valuesQuery.isPending || proposalsQuery.isPending))
	);
	const canGenerate = $derived(
		Boolean(selectedStudyId) && fields.length > 0 && !generateMutation.isPending
	);
	const hasNoStudies = $derived(!studiesQuery.isPending && studies.length === 0);

	let newFieldKey = $state('');
	let newFieldLabel = $state('');
	let newValueType = $state<ExtractionValueType>('text');
	let newFieldRequired = $state(false);
	let newFieldVersion = $state('1');
	let draftProposalId = $state<string | undefined>();
	let draftFields = $state<ExtractionDraftField[]>([]);
	let actionError = $state('');
	let actionStatus = $state<ActionStatus | undefined>();
	let fieldFormError = $state('');
	let actingProposalId = $state<string | undefined>();

	$effect(() => {
		if (!activeProposal) {
			draftProposalId = undefined;
			draftFields = [];
			return;
		}
		if (activeProposal.id !== draftProposalId) {
			draftProposalId = activeProposal.id;
			draftFields = activeProposal.payload.fields.map(draftFromAiField);
		}
	});

	function isDataExtractionProposal(
		proposal: AiProposalDto,
		studyId: string | undefined
	): proposal is DataExtractionProposal {
		return (
			Boolean(studyId) &&
			proposal.status === 'pending' &&
			proposal.task_kind === 'data_extraction' &&
			proposal.target_study_id === studyId &&
			proposal.payload.kind === 'data_extraction' &&
			proposal.payload.study_id === studyId
		);
	}

	async function selectStudy(studyId: string): Promise<void> {
		const search = updateExtractionLocation(page.url.searchParams, { studyId });
		let href: string = resolve('/projects/[projectId]/extraction', { projectId });
		href += search.toString() ? `?${search.toString()}` : '';
		await goto(href, { keepFocus: true, noScroll: true });
	}

	function resetActionError(): void {
		actionError = '';
		actionStatus = undefined;
	}

	function describeError(
		error: unknown,
		fallback: string
	): { message: string; status: ActionStatus } {
		if (error instanceof ApiError) {
			if (error.status === 503) {
				return {
					message:
						'The AI provider is unavailable. The proposal queue was not changed; try again later.',
					status: 'provider-unavailable'
				};
			}
			if (error.status === 409) {
				return {
					message:
						'This proposal or extraction schema changed elsewhere. Refresh the queue and review the current proposal.',
					status: 'conflict'
				};
			}
			return { message: error.message, status: 'error' };
		}
		return {
			message: error instanceof Error ? error.message : fallback,
			status: 'error'
		};
	}

	async function refreshExtractionData(): Promise<void> {
		await Promise.all([fieldsQuery.refetch(), valuesQuery.refetch(), proposalsQuery.refetch()]);
	}

	async function createField(): Promise<void> {
		fieldFormError = '';
		const fieldKey = newFieldKey.trim();
		const label = newFieldLabel.trim();
		const version = Number(newFieldVersion);
		if (!fieldKey || !label) {
			fieldFormError = 'Field key and label are required.';
			return;
		}
		if (!Number.isSafeInteger(version) || version < 1) {
			fieldFormError = 'Version must be a positive integer (1 or greater).';
			return;
		}
		const request: CreateExtractionFieldRequest = {
			field_key: fieldKey,
			label,
			required: newFieldRequired,
			value_type: newValueType,
			version
		};
		try {
			await createFieldMutation.mutateAsync({ projectId, data: request });
			newFieldKey = '';
			newFieldLabel = '';
			await queryClient.invalidateQueries({
				queryKey: getListExtractionFieldsQueryKey(projectId)
			});
			await fieldsQuery.refetch();
		} catch (error) {
			fieldFormError = describeError(
				error,
				'The extraction field could not be created.'
			).message;
		}
	}

	async function generateProposal(): Promise<void> {
		if (!selectedStudyId || !canGenerate) return;
		resetActionError();
		try {
			await generateMutation.mutateAsync({ projectId, studyId: selectedStudyId });
			await proposalsQuery.refetch();
		} catch (error) {
			const described = describeError(
				error,
				'The extraction proposal could not be generated.'
			);
			actionError = described.message;
			actionStatus = described.status;
		}
	}

	function updateDraft(
		fieldId: string,
		update: (draft: ExtractionDraftField) => ExtractionDraftField
	): void {
		draftFields = draftFields.map((draft) =>
			draft.field_id === fieldId ? update(draft) : draft
		);
	}

	function setDraftRationale(fieldId: string, event: Event): void {
		if (!(event.currentTarget instanceof HTMLTextAreaElement)) return;
		const rationale = event.currentTarget.value;
		updateDraft(fieldId, (draft) => ({ ...draft, rationale }));
	}

	function setDraftValue(fieldId: string, value: ExtractionDraftTypedValue): void {
		updateDraft(fieldId, (draft) => (draft.kind === 'value' ? { ...draft, value } : draft));
	}

	function setDraftTextValue(fieldId: string, event: Event): void {
		if (!(event.currentTarget instanceof HTMLInputElement)) return;
		setDraftValue(fieldId, { kind: 'text', value: event.currentTarget.value });
	}

	function setDraftNumberValue(fieldId: string, event: Event): void {
		if (!(event.currentTarget instanceof HTMLInputElement)) return;
		setDraftValue(fieldId, { kind: 'number', value: event.currentTarget.value });
	}

	function setDraftDateValue(fieldId: string, event: Event): void {
		if (!(event.currentTarget instanceof HTMLInputElement)) return;
		setDraftValue(fieldId, { kind: 'date', value: event.currentTarget.value });
	}

	function markInsufficient(fieldId: string): void {
		updateDraft(fieldId, (draft) => ({
			field_id: draft.field_id,
			field_version: draft.field_version,
			kind: 'insufficient_evidence',
			rationale: draft.rationale
		}));
	}

	function defaultReviewedValue(valueType: string): ExtractionDraftTypedValue | undefined {
		if (!isExtractionValueType(valueType)) return undefined;
		switch (valueType) {
			case 'text':
				return { kind: 'text', value: 'Reviewed value' };
			case 'number':
				return { kind: 'number', value: '0' };
			case 'boolean':
				return { kind: 'boolean', value: false };
			case 'date':
				return { kind: 'date', value: '1970-01-01' };
			default: {
				const exhaustive: never = valueType;
				return exhaustive;
			}
		}
	}

	function enterReviewedValue(fieldId: string): void {
		const original = originalField(fieldId);
		const field = fields.find((candidate) => candidate.id === fieldId);
		if (original?.kind !== 'value' || !field) return;
		const value = defaultReviewedValue(field.value_type);
		if (!value) return;
		updateDraft(fieldId, (draft) => ({
			field_id: draft.field_id,
			field_version: draft.field_version,
			kind: 'value',
			rationale: 'Reviewed by a human against the cited source.',
			source: original.source,
			value
		}));
	}

	function restoreOriginalValue(fieldId: string): void {
		const original = activeProposal?.payload.fields.find((field) => field.field_id === fieldId);
		if (original?.kind !== 'value') return;
		updateDraft(fieldId, () => draftFromAiField(original));
	}

	function fieldLabel(fieldId: string): string {
		return fields.find((field) => field.id === fieldId)?.label ?? fieldId;
	}

	function originalField(fieldId: string): AiExtractedFieldDto | undefined {
		return activeProposal?.payload.fields.find((field) => field.field_id === fieldId);
	}

	function typedValueLabel(value: ExtractionValueDto['value']): string {
		switch (value.kind) {
			case 'text':
				return value.value;
			case 'number':
				return String(value.value);
			case 'boolean':
				return value.value ? 'Yes' : 'No';
			case 'date':
				return value.value;
			default: {
				const exhaustive: never = value;
				return exhaustive;
			}
		}
	}

	function valueTypeLabel(valueType: string): string {
		return isExtractionValueType(valueType) ? valueType : `unsupported (${valueType})`;
	}

	function extractionValueKey(value: ExtractionValueDto): string {
		return `${value.id}:${value.field_definition_id}:${value.field_definition_version}`;
	}

	function evidenceLabel(evidence: {
		report_id: string;
		page: number;
		document_block_id: string;
	}): string {
		return `Report ${evidence.report_id} · page ${evidence.page} · block ${evidence.document_block_id}`;
	}

	async function decideProposal(
		proposal: DataExtractionProposal,
		decision: 'accept' | 'reject'
	): Promise<void> {
		if (actingProposalId) return;
		resetActionError();
		actingProposalId = proposal.id;
		try {
			if (decision === 'reject') {
				await decideMutation.mutateAsync({
					projectId,
					proposalId: proposal.id,
					data: {
						decision: 'reject',
						reason: 'Human reviewer rejected the data extraction proposal.'
					}
				});
			} else {
				const validation = validateExtractionDrafts(fields, draftFields);
				if (validation) {
					actionError = validation;
					actionStatus = 'validation';
					return;
				}
				const serialized = serializeExtractionDrafts(draftFields);
				if (!serialized.ok) {
					actionError = serialized.message;
					actionStatus = 'validation';
					return;
				}
				const reviewedPayload: AiReviewedProposalPayload = {
					kind: 'data_extraction',
					study_id: proposal.payload.study_id,
					fields: serialized.fields
				};
				await decideMutation.mutateAsync({
					projectId,
					proposalId: proposal.id,
					data: {
						decision: 'accept',
						reason: 'Human reviewer accepted the edited data extraction proposal.',
						reviewed_payload: reviewedPayload
					}
				});
			}
			await queryClient.invalidateQueries({
				queryKey: getListStudyExtractionValuesQueryKey(projectId, proposal.payload.study_id)
			});
			await refreshExtractionData();
		} catch (error) {
			const described = describeError(error, `The proposal could not be ${decision}ed.`);
			actionError = described.message;
			actionStatus = described.status;
			await proposalsQuery.refetch();
		} finally {
			actingProposalId = undefined;
		}
	}
</script>

<svelte:head>
	<title>Extraction · DeepRef</title>
	<meta
		name="description"
		content="Review AI extraction proposals and approve evidence-linked study data."
	/>
</svelte:head>

<main class="mx-auto flex max-w-7xl flex-col gap-6 p-6">
	<div class="flex flex-wrap items-start justify-between gap-4">
		<div>
			<p class="text-sm text-muted-foreground">Structured study data</p>
			<h1 class="text-3xl font-semibold tracking-tight">Extraction</h1>
			<p class="mt-2 max-w-3xl text-muted-foreground">
				AI extraction is proposal-only. Review typed values and their exact source blocks
				before an audited approval writes scientific state.
			</p>
		</div>
		<Badge variant="outline" data-testid="extraction-proposal-only"
			>Proposal only · audited approval</Badge
		>
	</div>

	{#if queryError}
		<Alert.Root variant="destructive" role="alert">
			<Alert.Title>Extraction data unavailable</Alert.Title>
			<Alert.Description>{queryError}</Alert.Description>
		</Alert.Root>
	{/if}
	{#if actionError}
		<Alert.Root
			variant={actionStatus === 'validation' ? 'default' : 'destructive'}
			role="alert"
		>
			{#if actionStatus === 'provider-unavailable'}<Alert.Title
					>AI provider unavailable</Alert.Title
				>
			{:else if actionStatus === 'conflict'}<Alert.Title>Review conflict</Alert.Title>
			{:else if actionStatus === 'validation'}<Alert.Title>Review needs attention</Alert.Title
				>
			{:else}<Alert.Title>Extraction action failed</Alert.Title>{/if}
			<Alert.Description>{actionError}</Alert.Description>
		</Alert.Root>
	{/if}

	<Card.Root>
		<Card.Header>
			<Card.Title>Study</Card.Title>
			<Card.Description
				>Selection is stored in the URL so the review is refresh-safe.</Card.Description
			>
		</Card.Header>
		<Card.Content>
			{#if studiesQuery.isPending}
				<Skeleton class="h-9 w-full" />
			{:else if hasNoStudies}
				<Empty.Root class="border-0 p-0">
					<Empty.Media variant="icon"><FileSearch /></Empty.Media>
					<Empty.Header>
						<Empty.Title>No studies yet</Empty.Title>
						<Empty.Description
							>Create or group a study before extracting data.</Empty.Description
						>
					</Empty.Header>
				</Empty.Root>
			{:else}
				<Select.Root
					type="single"
					value={selectedStudyId ?? ''}
					onValueChange={(value) => value && void selectStudy(value)}
				>
					<Select.Trigger id="extraction-study" class="w-full max-w-xl"
						>{selectedStudyLabel}</Select.Trigger
					>
					<Select.Content>
						<Select.Group>
							{#each studies as study (study.id)}
								<Select.Item value={study.id} label={study.title}
									>{study.title}</Select.Item
								>
							{/each}
						</Select.Group>
					</Select.Content>
				</Select.Root>
			{/if}
		</Card.Content>
	</Card.Root>

	<div class="grid gap-6 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.35fr)]">
		<Card.Root>
			<Card.Header>
				<Card.Title>Current extraction fields</Card.Title>
				<Card.Description
					>Versioned fields define the schema used by new proposals.</Card.Description
				>
			</Card.Header>
			<Card.Content class="flex flex-col gap-4">
				{#if fieldsQuery.isPending}
					<div class="flex flex-col gap-3">
						<Skeleton class="h-5 w-2/3" /><Skeleton class="h-16 w-full" />
					</div>
				{:else if fields.length === 0}
					<Empty.Root class="border-0 p-0">
						<Empty.Media variant="icon"><Plus /></Empty.Media>
						<Empty.Header>
							<Empty.Title>No extraction fields</Empty.Title>
							<Empty.Description
								>Add a field to define the study data to extract.</Empty.Description
							>
						</Empty.Header>
					</Empty.Root>
				{:else}
					<div class="flex flex-col gap-2" data-testid="extraction-fields">
						{#each fields as field (field.id)}
							<div
								class="flex flex-wrap items-center justify-between gap-2 rounded-md border p-3"
								data-testid={`extraction-field-${field.field_key}`}
							>
								<div>
									<p class="font-medium">{field.label}</p>
									<p class="text-xs text-muted-foreground">
										{field.field_key} · {valueTypeLabel(field.value_type)} · v{field.version}
									</p>
								</div>
								{#if field.required}<Badge variant="secondary">Required</Badge
									>{:else}<Badge variant="outline">Optional</Badge>{/if}
							</div>
						{/each}
					</div>
				{/if}

				<Separator />
				<form
					class="flex flex-col gap-4"
					onsubmit={(event) => {
						event.preventDefault();
						void createField();
					}}
				>
					<div>
						<h3 class="font-medium">Add current field</h3>
						<p class="text-xs text-muted-foreground">
							Use a new version when the schema meaning changes.
						</p>
					</div>
					{#if fieldFormError}<p class="text-sm text-destructive" role="alert">
							{fieldFormError}
						</p>{/if}
					<Field.FieldGroup>
						<Field.Field>
							<Field.FieldLabel for="extraction-field-key">Field key</Field.FieldLabel
							>
							<Input
								id="extraction-field-key"
								bind:value={newFieldKey}
								placeholder="sample_size"
							/>
						</Field.Field>
						<Field.Field>
							<Field.FieldLabel for="extraction-field-label">Label</Field.FieldLabel>
							<Input
								id="extraction-field-label"
								bind:value={newFieldLabel}
								placeholder="Sample size"
							/>
						</Field.Field>
						<div class="grid gap-4 sm:grid-cols-2">
							<Field.Field>
								<Field.FieldLabel for="extraction-field-type"
									>Value type</Field.FieldLabel
								>
								<Select.Root
									type="single"
									value={newValueType}
									onValueChange={(value) => {
										if (value && isExtractionValueType(value))
											newValueType = value;
									}}
								>
									<Select.Trigger id="extraction-field-type"
										>{newValueType}</Select.Trigger
									>
									<Select.Content>
										<Select.Group>
											{#each EXTRACTION_VALUE_TYPES as value (value)}
												<Select.Item {value} label={value}
													>{value}</Select.Item
												>
											{/each}
										</Select.Group>
									</Select.Content>
								</Select.Root>
							</Field.Field>
							<Field.Field>
								<Field.FieldLabel for="extraction-field-version"
									>Version</Field.FieldLabel
								>
								<Input
									id="extraction-field-version"
									type="number"
									min="1"
									step="1"
									bind:value={newFieldVersion}
								/>
							</Field.Field>
						</div>
						<Field.Field orientation="horizontal">
							<Checkbox
								id="extraction-field-required"
								bind:checked={newFieldRequired}
							/>
							<Field.FieldLabel for="extraction-field-required" class="font-normal"
								>Required field</Field.FieldLabel
							>
						</Field.Field>
					</Field.FieldGroup>
					<Button type="submit" disabled={createFieldMutation.isPending}>
						{#if createFieldMutation.isPending}<Spinner
								data-icon="inline-start"
							/>{:else}<Plus data-icon="inline-start" />{/if}
						{createFieldMutation.isPending ? 'Adding field…' : 'Add field'}
					</Button>
				</form>
			</Card.Content>
		</Card.Root>

		<Card.Root>
			<Card.Header class="gap-3">
				<div class="flex flex-wrap items-center justify-between gap-2">
					<div class="flex items-center gap-2">
						<Brain aria-hidden="true" /><Card.Title>AI extraction proposal</Card.Title>
					</div>
					<Button
						variant="outline"
						size="sm"
						disabled={!canGenerate}
						onclick={() => void generateProposal()}
					>
						{#if generateMutation.isPending}<Spinner
								data-icon="inline-start"
							/>{:else}<RefreshCw data-icon="inline-start" />{/if}
						{generateMutation.isPending ? 'Generating…' : 'Generate proposal'}
					</Button>
				</div>
				<Card.Description>
					{#if !selectedStudyId}
						Select a study to load its pending proposal and accepted values.
					{:else if fields.length === 0}
						Create at least one current field before generating a proposal.
					{:else}
						Suggestions are scoped to {selectedStudyLabel}; accept and reject decisions
						are audited.
					{/if}
				</Card.Description>
			</Card.Header>
			<Card.Content class="flex flex-col gap-4">
				{#if !selectedStudyId}
					<Empty.Root class="border-0 p-0">
						<Empty.Media variant="icon"><FileSearch /></Empty.Media>
						<Empty.Header
							><Empty.Title>Select a study</Empty.Title><Empty.Description
								>Pending proposals and accepted values are study-scoped.</Empty.Description
							></Empty.Header
						>
					</Empty.Root>
				{:else if loading}
					<div class="flex flex-col gap-3" aria-label="Loading extraction">
						<Skeleton class="h-5 w-2/3" /><Skeleton class="h-24 w-full" /><Skeleton
							class="h-10 w-1/2"
						/>
					</div>
				{:else if !activeProposal}
					<Empty.Root class="border-0 p-0">
						<Empty.Media variant="icon"><Brain /></Empty.Media>
						<Empty.Header
							><Empty.Title>No pending proposal</Empty.Title><Empty.Description
								>Generate a grounded proposal to review typed values against source
								blocks.</Empty.Description
							></Empty.Header
						>
					</Empty.Root>
				{:else}
					{@const proposal = activeProposal}
					<div class="flex flex-wrap items-center gap-2">
						<Badge variant="secondary">pending</Badge>
						<span class="text-xs text-muted-foreground"
							>{proposal.provider} / {proposal.model} · prompt {proposal.prompt_version}</span
						>
					</div>
					<div class="flex flex-col gap-4" data-testid="extraction-proposal-editor">
						{#each draftFields as draft (draft.field_id)}
							{@const original = originalField(draft.field_id)}
							{@const field = fields.find(
								(candidate) => candidate.id === draft.field_id
							)}
							<div
								class="rounded-lg border p-4"
								data-testid={`extraction-proposal-field-${draft.field_id}`}
							>
								<div class="flex flex-wrap items-start justify-between gap-2">
									<div>
										<h3 class="font-medium">
											{field?.label ?? fieldLabel(draft.field_id)}
										</h3>
										<p class="text-xs text-muted-foreground">
											Field {draft.field_id} · version {draft.field_version}
										</p>
									</div>
									{#if draft.kind === 'insufficient_evidence'}<Badge
											variant="outline">Insufficient evidence</Badge
										>{:else}<Badge variant="secondary">{draft.value.kind}</Badge
										>{/if}
								</div>
								<Field.FieldGroup class="mt-4">
									<Field.Field>
										<Field.FieldLabel
											for={`extraction-rationale-${draft.field_id}`}
											>Reviewer rationale</Field.FieldLabel
										>
										<Textarea
											id={`extraction-rationale-${draft.field_id}`}
											rows={2}
											value={draft.rationale}
											oninput={(event) =>
												setDraftRationale(draft.field_id, event)}
										/>
									</Field.Field>
									{#if draft.kind === 'value'}
										{#if draft.value.kind === 'text'}
											<Field.Field>
												<Field.FieldLabel
													for={`extraction-value-${draft.field_id}`}
													>Text value</Field.FieldLabel
												>
												<Input
													id={`extraction-value-${draft.field_id}`}
													value={draft.value.value}
													oninput={(event) =>
														setDraftTextValue(draft.field_id, event)}
												/>
											</Field.Field>
										{:else if draft.value.kind === 'number'}
											<Field.Field>
												<Field.FieldLabel
													for={`extraction-value-${draft.field_id}`}
													>Number value</Field.FieldLabel
												>
												<Input
													id={`extraction-value-${draft.field_id}`}
													type="number"
													step="any"
													value={draft.value.value}
													oninput={(event) =>
														setDraftNumberValue(draft.field_id, event)}
												/>
											</Field.Field>
										{:else if draft.value.kind === 'boolean'}
											<Field.Field orientation="horizontal">
												<Checkbox
													id={`extraction-value-${draft.field_id}`}
													checked={draft.value.value}
													onCheckedChange={(checked) =>
														setDraftValue(draft.field_id, {
															kind: 'boolean',
															value: checked === true
														})}
												/>
												<Field.FieldLabel
													for={`extraction-value-${draft.field_id}`}
													class="font-normal"
													>Boolean value</Field.FieldLabel
												>
											</Field.Field>
										{:else if draft.value.kind === 'date'}
											<Field.Field>
												<Field.FieldLabel
													for={`extraction-value-${draft.field_id}`}
													>ISO date value</Field.FieldLabel
												>
												<Input
													id={`extraction-value-${draft.field_id}`}
													type="date"
													value={draft.value.value}
													oninput={(event) =>
														setDraftDateValue(draft.field_id, event)}
												/>
											</Field.Field>
										{/if}
									{:else}
										<p class="text-sm text-muted-foreground">
											This field will not write a value unless a reviewer
											supplies a supported source-backed value.
										</p>
										{#if field?.required}
											<p class="text-sm text-destructive" role="status">
												Required field: insufficiency cannot be accepted.
												Enter a source-backed value or generate a new
												grounded proposal.
											</p>
										{/if}
									{/if}
								</Field.FieldGroup>

								{#if draft.kind === 'value'}
									<div class="mt-4 rounded-md bg-muted/50 p-3 text-sm">
										<div class="flex items-center gap-2 font-medium">
											<FileSearch aria-hidden="true" />Evidence
										</div>
										<a
											class="mt-2 block text-primary underline underline-offset-4"
											href={resolve(
												`/projects/${encodeURIComponent(projectId)}/screening/full-text${buildExtractionEvidenceSearch(draft.source)}`
											)}
											data-testid="extraction-evidence-link"
										>
											{evidenceLabel(draft.source)}
										</a>
										<p class="mt-1 text-xs break-all text-muted-foreground">
											document {draft.source.document_id} · parser {draft
												.source.parser_version} · hash {draft.source
												.content_hash}
										</p>
									</div>
									<Button
										variant="outline"
										size="sm"
										class="mt-3"
										onclick={() => markInsufficient(draft.field_id)}
									>
										<X data-icon="inline-start" />Mark insufficient evidence
									</Button>
								{:else if original?.kind === 'value'}
									<Button
										variant="outline"
										size="sm"
										class="mt-3"
										onclick={() => enterReviewedValue(draft.field_id)}
										data-testid={`enter-reviewed-value-${draft.field_id}`}
									>
										<Plus data-icon="inline-start" />Enter reviewed value
									</Button>
									<Button
										variant="outline"
										size="sm"
										class="mt-3"
										onclick={() => restoreOriginalValue(draft.field_id)}
									>
										<RefreshCw data-icon="inline-start" />Restore proposed value
									</Button>
								{:else}
									<p class="mt-3 text-sm text-muted-foreground" role="status">
										No source block is attached to this insufficient-evidence
										proposal. Entering a value is unavailable; generate a new
										grounded proposal before accepting a value.
									</p>
								{/if}
							</div>
						{/each}
					</div>
					<div class="flex flex-wrap gap-2 border-t pt-4">
						<Button
							disabled={Boolean(actingProposalId)}
							onclick={() => void decideProposal(proposal, 'accept')}
						>
							{#if actingProposalId === proposal.id}<Spinner
									data-icon="inline-start"
								/>{:else}<Check data-icon="inline-start" />{/if}
							Accept reviewed values
						</Button>
						<Button
							variant="outline"
							disabled={Boolean(actingProposalId)}
							onclick={() => void decideProposal(proposal, 'reject')}
						>
							<X data-icon="inline-start" />Reject proposal
						</Button>
					</div>
				{/if}
			</Card.Content>
		</Card.Root>
	</div>

	{#if selectedStudyId}
		<Card.Root>
			<Card.Header>
				<Card.Title>Accepted values</Card.Title>
				<Card.Description
					>Only audited, approved values appear here. Grouping changes do not rewrite
					their provenance.</Card.Description
				>
			</Card.Header>
			<Card.Content>
				{#if valuesQuery.isPending}
					<div class="flex flex-col gap-3">
						<Skeleton class="h-5 w-2/3" /><Skeleton class="h-16 w-full" />
					</div>
				{:else if values.length === 0}
					<Empty.Root class="border-0 p-0">
						<Empty.Media variant="icon"><Check /></Empty.Media>
						<Empty.Header
							><Empty.Title>No accepted values</Empty.Title><Empty.Description
								>Approved extraction values will be listed with their immutable
								source details.</Empty.Description
							></Empty.Header
						>
					</Empty.Root>
				{:else}
					<div class="flex flex-col gap-3" data-testid="accepted-extraction-values">
						{#each values as value (extractionValueKey(value))}
							<div
								class="rounded-md border p-3"
								data-testid={`accepted-extraction-value-${value.field_definition_id}`}
							>
								<div class="flex flex-wrap items-start justify-between gap-2">
									<div>
										<p class="font-medium">
											{fieldLabel(value.field_definition_id)}
										</p>
										<p class="text-sm">{typedValueLabel(value.value)}</p>
									</div>
									<Badge variant="secondary">approved</Badge>
								</div>
								<p class="mt-2 text-xs text-muted-foreground">{value.rationale}</p>
								<a
									class="mt-2 block text-sm text-primary underline underline-offset-4"
									href={resolve(
										`/projects/${encodeURIComponent(projectId)}/screening/full-text${buildExtractionEvidenceSearch(
											{
												report_id: value.report_id,
												document_id: value.source_document_id,
												document_block_id: value.source_block_id,
												page: value.source_page,
												parser_version: value.source_parser_version,
												content_hash: value.source_content_hash
											}
										)}`
									)}
								>
									{evidenceLabel({
										report_id: value.report_id,
										page: value.source_page,
										document_block_id: value.source_block_id
									})}
								</a>
								<p class="mt-1 text-xs break-all text-muted-foreground">
									document {value.source_document_id} · parser {value.source_parser_version}
									· hash {value.source_content_hash}
								</p>
							</div>
						{/each}
					</div>
				{/if}
			</Card.Content>
		</Card.Root>
	{/if}
</main>
