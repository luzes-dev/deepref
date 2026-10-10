<script lang="ts">
	import * as Resizable from '@deepref/ui/resizable';
	import { MediaQuery } from 'svelte/reactivity';
	import {
		createListDocumentBlocks,
		createListReportDocuments
	} from '#lib/api/generated/documents/documents.js';
	import { fullTextUrlString } from '#lib/features/full-text/url.js';
	import EvidenceLabel from '#lib/features/evidence/EvidenceLabel.svelte';
	import CitationLabel from './CitationLabel.svelte';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import {
		createDecideAiProposal,
		createGenerateDataExtractionSuggestion,
		createGetAiStatus,
		createListAiProposals
	} from '#lib/api/generated/ai/ai.js';
	import {
		createClearExtractionValue,
		createConfirmExtractionValue,
		createCreateExtractionField,
		createUpdateExtractionField,
		createRecordExtractionValue,
		createListExtractionFields,
		createListStudyExtractionValues,
		getListExtractionFieldsQueryKey,
		getListStudyExtractionValuesQueryKey
	} from '#lib/api/generated/extraction/extraction.js';
	import { ApiError } from '#lib/api/custom-fetch.js';
	import {
		notifyError,
		notifySuccess,
		notifyWarning
	} from '#lib/features/notifications/toast.js';
	import {
		createGetProjectStudy,
		createListProjectStudies
	} from '#lib/api/generated/studies/studies.js';
	import type {
		AiExtractedFieldDto,
		AiProposalDto,
		AiReviewedProposalPayload,
		CreateExtractionFieldRequest,
		ExtractionValueDto
	} from '#lib/api/generated/models/index.js';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { StatePanel, Surface } from '@deepref/ui/layout';
	import PageTemplate from '#lib/shell/PageTemplate.svelte';
	import * as Alert from '@deepref/ui/alert';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import { Checkbox } from '@deepref/ui/checkbox';
	import type { ExtractionFieldDto } from '#lib/api/generated/models/index.js';
	import * as Field from '@deepref/ui/field';
	import { Input } from '@deepref/ui/input';
	import * as Select from '@deepref/ui/select';
	import { Spinner } from '@deepref/ui/spinner';
	import { Textarea } from '@deepref/ui/textarea';
	import {
		ArrowLeft,
		Brain,
		Check,
		FileSearch,
		Pencil,
		Plus,
		RefreshCw,
		Settings2,
		X
	} from '@lucide/svelte';
	import { Skeleton } from '@deepref/ui/skeleton';
	import { ReviewRunObserver } from '#lib/features/ai-assistance/review-run-observer.svelte.js';
	import {
		buildExtractionEvidenceSearch,
		deriveFieldKey,
		describeFieldEditError,
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
	const location = $derived(
		parseExtractionLocation(new URLSearchParams(page.url.searchParams.toString()))
	);
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
	const updateFieldMutation = createUpdateExtractionField();
	const recordValueMutation = createRecordExtractionValue();
	const clearValueMutation = createClearExtractionValue();
	const confirmValueMutation = createConfirmExtractionValue();
	const generateMutation = createGenerateDataExtractionSuggestion();
	const decideMutation = createDecideAiProposal();
	const reviewRun = new ReviewRunObserver(
		() => projectId,
		async () => {
			await proposalsQuery.refetch();
		}
	);

	const studies = $derived(studiesQuery.data?.data.items ?? []);
	const selectedStudy = $derived(studies.find((study) => study.id === selectedStudyId));
	const selectedStudyQuery = createGetProjectStudy(
		() => projectId,
		() => selectedStudyId ?? '',
		() => ({ query: { enabled: Boolean(selectedStudyId) } })
	);
	const studyReports = $derived(selectedStudyQuery.data?.data.reports ?? []);
	const wide = new MediaQuery('(min-width: 1024px)');
	let evidenceReportId = $state('');
	const sourceReportId = $derived(
		studyReports.find((report) => report.report_id === evidenceReportId)?.report_id ??
			studyReports[0]?.report_id ??
			''
	);
	const sourceDocuments = createListReportDocuments(
		() => projectId,
		() => sourceReportId,
		() => ({ limit: 100 }),
		() => ({ query: { enabled: Boolean(sourceReportId) } })
	);
	const sourceDocumentId = $derived(sourceDocuments.data?.data[0]?.id ?? '');
	const sourceBlocks = createListDocumentBlocks(
		() => projectId,
		() => sourceReportId,
		() => sourceDocumentId,
		() => ({ limit: 100 }),
		() => ({ query: { enabled: Boolean(sourceReportId && sourceDocumentId) } })
	);
	const fields = $derived(fieldsQuery.data?.data ?? []);
	const values = $derived(valuesQuery.data?.data ?? []);
	const toVerifyCount = $derived(values.filter((value) => value.needs_verification).length);
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
	let providerFailed = $state(false);
	// Known up front from the workspace AI status; a 503 on generate also flips it.
	const aiStatusQuery = createGetAiStatus();
	const providerUnavailable = $derived(
		providerFailed || aiStatusQuery.data?.data.suggestions_available === false
	);
	const canGenerate = $derived(
		Boolean(selectedStudyId) &&
			fields.length > 0 &&
			!providerUnavailable &&
			!generateMutation.isPending &&
			!reviewRun.isActive
	);
	const hasNoStudies = $derived(!studiesQuery.isPending && studies.length === 0);
	type ExtractionPageState = 'loading' | 'error' | 'empty' | 'ready';
	const pageState = $derived<ExtractionPageState>(
		queryError ? 'error' : loading ? 'loading' : hasNoStudies ? 'empty' : 'ready'
	);

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
	const proposalDirty = $derived(
		Boolean(
			activeProposal &&
			draftProposalId === activeProposal.id &&
			JSON.stringify(draftFields) !==
				JSON.stringify(activeProposal.payload.fields.map(draftFromAiField))
		)
	);

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

	async function selectStudy(studyId: string, replaceState = false): Promise<void> {
		const search = updateExtractionLocation(
			new URLSearchParams(page.url.searchParams.toString()),
			{ studyId }
		);
		let href: string = resolve('/projects/[projectId]/extraction', { projectId });
		href += search.toString() ? `?${search.toString()}` : '';
		await goto(href, { reset: false, replace: replaceState });
	}

	let view = $state<'data' | 'fields'>('data');
	$effect(() => {
		const first = studies[0];
		if (!selectedStudyId && first) void selectStudy(first.id, true);
	});
	const derivedFieldKey = $derived(deriveFieldKey(newFieldLabel));
	type SheetRow = {
		key: string;
		field: (typeof fields)[number] | undefined;
		label: string;
		required: boolean;
		value: (typeof values)[number] | undefined;
	};
	const sheetRows = $derived.by((): SheetRow[] => {
		const rows: SheetRow[] = fields.map((field) => ({
			key: field.id,
			field,
			label: field.label,
			required: field.required,
			value: values.find((value) => value.field_definition_id === field.id)
		}));
		for (const value of values) {
			if (fields.some((field) => field.id === value.field_definition_id)) continue;
			rows.push({
				key: extractionValueKey(value),
				field: undefined,
				label: fieldLabel(value.field_definition_id),
				required: false,
				value
			});
		}
		return rows;
	});
	const filledCount = $derived(sheetRows.filter((row) => row.value).length);

	function resetActionError(): void {
		actionError = '';
		actionStatus = undefined;
	}

	function notifyExtractionError(described: { message: string; status: ActionStatus }): void {
		const titles: Record<string, string> = {
			'provider-unavailable': 'AI provider unavailable',
			conflict: 'Review conflict'
		};
		const title = titles[described.status] ?? 'Extraction action failed';
		if (described.status === 'conflict' || described.status === 'provider-unavailable') {
			notifyWarning(title, described.message);
			return;
		}
		notifyError(title, undefined, described.message);
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

	let editingDefinitionId = $state<string | null>(null);
	let editFieldLabel = $state('');
	let editFieldType = $state<ExtractionValueType>('text');
	let editFieldRequired = $state(false);
	let editFieldError = $state('');

	function startFieldEdit(field: ExtractionFieldDto): void {
		editingDefinitionId = field.id;
		editFieldLabel = field.label;
		editFieldType = isExtractionValueType(field.value_type) ? field.value_type : 'text';
		editFieldRequired = field.required;
		editFieldError = '';
	}

	async function saveFieldEdit(field: ExtractionFieldDto): Promise<void> {
		const label = editFieldLabel.trim();
		if (!label) {
			editFieldError = 'Enter a label for the field.';
			return;
		}
		editFieldError = '';
		try {
			await updateFieldMutation.mutateAsync({
				projectId,
				fieldId: field.id,
				data: { label, value_type: editFieldType, required: editFieldRequired }
			});
			editingDefinitionId = null;
			await queryClient.invalidateQueries({
				queryKey: getListExtractionFieldsQueryKey(projectId)
			});
			await fieldsQuery.refetch();
		} catch (error) {
			editFieldError = describeFieldEditError(error);
		}
	}

	async function createField(): Promise<void> {
		fieldFormError = '';
		const fieldKey = newFieldKey.trim() || derivedFieldKey;
		const label = newFieldLabel.trim();
		const version = Number(newFieldVersion);
		if (!fieldKey || !label) {
			fieldFormError = 'A label is required.';
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
			newValueType = 'text';
			newFieldRequired = false;
			newFieldVersion = '1';
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
			const response = await generateMutation.mutateAsync({
				projectId,
				studyId: selectedStudyId
			});
			await reviewRun.observe(response.data);
		} catch (error) {
			const described = describeError(
				error,
				'The extraction proposal could not be generated.'
			);
			if (described.status === 'provider-unavailable') {
				providerFailed = true;
				return;
			}
			notifyExtractionError(described);
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

	type EditSource = { documentId: string; blockId: string; page: number };
	let editingFieldId = $state<string | undefined>();
	let editText = $state('');
	let editBoolean = $state(false);
	let editRationale = $state('');
	let editSource = $state<EditSource | undefined>();
	let editError = $state('');
	const savingValue = $derived(recordValueMutation.isPending || clearValueMutation.isPending);

	function startEdit(field: SheetRow['field'], current: ExtractionValueDto | undefined): void {
		if (!field || savingValue) return;
		editingFieldId = field.id;
		editError = '';
		editText = current && current.value.kind !== 'boolean' ? String(current.value.value) : '';
		editBoolean = current?.value.kind === 'boolean' ? current.value.value : false;
		editRationale = current?.rationale ?? '';
		editSource =
			current?.source_document_id && current.source_block_id && current.source_page
				? {
						documentId: current.source_document_id,
						blockId: current.source_block_id,
						page: current.source_page
					}
				: undefined;
	}

	function cancelEdit(): void {
		editingFieldId = undefined;
		editError = '';
	}

	function buildEditValue(valueType: string): ExtractionValueDto['value'] | string {
		const text = String(editText).trim();
		switch (valueType) {
			case 'text':
				return text ? { kind: 'text', value: text } : 'Enter a value.';
			case 'number': {
				const parsed = Number(text);
				return text && Number.isFinite(parsed)
					? { kind: 'number', value: parsed }
					: 'Enter a valid number.';
			}
			case 'boolean':
				return { kind: 'boolean', value: editBoolean };
			case 'date':
				return /^\d{4}-\d{2}-\d{2}$/.test(text)
					? { kind: 'date', value: text }
					: 'Enter a valid date.';

			default:
				return 'This field type cannot be edited here.';
		}
	}

	async function saveEdit(field: NonNullable<SheetRow['field']>): Promise<void> {
		if (!selectedStudyId || savingValue) return;
		const value = buildEditValue(field.value_type);
		if (typeof value === 'string') {
			editError = value;
			return;
		}
		editError = '';
		try {
			await recordValueMutation.mutateAsync({
				projectId,
				studyId: selectedStudyId,
				fieldId: field.id,
				data: {
					value,
					rationale: editRationale.trim() || undefined,
					source: editSource
						? {
								document_id: editSource.documentId,
								document_block_id: editSource.blockId
							}
						: undefined
				}
			});
			await valuesQuery.refetch();
			cancelEdit();
		} catch (error) {
			editError = describeError(error, 'The value could not be saved.').message;
		}
	}

	async function confirmValue(valueId: string): Promise<void> {
		if (!selectedStudyId || confirmValueMutation.isPending) return;
		try {
			await confirmValueMutation.mutateAsync({
				projectId,
				studyId: selectedStudyId,
				valueId
			});
			await valuesQuery.refetch();
			notifySuccess('Value confirmed', 'It no longer needs checking.');
		} catch (error) {
			notifyError('Could not confirm this value', error, 'The value was not confirmed.');
		}
	}

	async function clearEdit(field: NonNullable<SheetRow['field']>): Promise<void> {
		if (!selectedStudyId || savingValue) return;
		try {
			await clearValueMutation.mutateAsync({
				projectId,
				studyId: selectedStudyId,
				fieldId: field.id
			});
			await valuesQuery.refetch();
			cancelEdit();
		} catch (error) {
			editError = describeError(error, 'The value could not be cleared.').message;
		}
	}

	function citeBlock(block: { id: string; page_number: number }): void {
		if (!editingFieldId || !sourceDocumentId) return;
		editSource = { documentId: sourceDocumentId, blockId: block.id, page: block.page_number };
	}

	function onEditKeydown(event: KeyboardEvent): void {
		if (event.key === 'Escape') {
			event.preventDefault();
			cancelEdit();
		}
	}

	function valueTypeLabel(valueType: string): string {
		return isExtractionValueType(valueType) ? valueType : `unsupported (${valueType})`;
	}

	function extractionValueKey(value: ExtractionValueDto): string {
		return `${value.id}:${value.field_definition_id}:${value.field_definition_version}`;
	}

	function reportTitle(reportId: string): string | null {
		return studyReports.find((candidate) => candidate.report_id === reportId)?.title ?? null;
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
			notifyExtractionError(described);
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

{#snippet studyList()}
	<div class="flex items-center justify-between gap-2 px-4 pt-4 pb-2">
		<h2 class="text-sm font-semibold">
			Studies <span class="font-normal text-muted-foreground tabular-nums"
				>{studies.length}</span
			>
		</h2>
	</div>
	<div class="min-h-0 flex-1 overflow-y-auto">
		{#if studiesQuery.isPending}
			<div class="flex flex-col gap-2 p-2" data-testid="extraction-study-loading">
				{#each { length: 3 }, index (index)}<Skeleton class="h-10 w-full" />{/each}
			</div>
		{:else if hasNoStudies}
			<div class="flex flex-col items-start gap-2 px-4" data-testid="extraction-study-empty">
				<p class="text-sm text-muted-foreground">
					No studies yet. Group included reports into studies before extracting data.
				</p>
				<Button
					variant="outline"
					size="sm"
					href={resolve('/projects/[projectId]/studies', { projectId })}
					>Open studies</Button
				>
			</div>
		{:else}
			<ol class="flex flex-col p-1.5" aria-label="Studies to extract">
				{#each studies as study (study.id)}
					{@const selected = study.id === selectedStudyId}
					<li>
						<button
							type="button"
							class={[
								'flex w-full flex-col gap-0.5 rounded-md px-2.5 py-2 text-left text-sm transition-colors hover:bg-muted focus-visible:outline-2 focus-visible:outline-ring',
								selected && 'bg-accent shadow-inset-accent'
							]}
							aria-current={selected ? 'true' : undefined}
							onclick={() => void selectStudy(study.id)}
						>
							<span class={['leading-snug', selected && 'font-medium']}
								>{study.title}</span
							>
							<span class="text-xs text-muted-foreground"
								>{study.design_label ?? 'Design not classified'}</span
							>
						</button>
					</li>
				{/each}
			</ol>
		{/if}
	</div>
{/snippet}

{#snippet mobileStudyPicker()}
	{#if studiesQuery.isPending}
		<div data-testid="extraction-study-loading"><Skeleton class="h-10 w-full" /></div>
	{:else if hasNoStudies}
		<p class="text-sm text-muted-foreground" data-testid="extraction-study-empty">
			No studies yet. Group included reports into studies before extracting data.
		</p>
	{:else}
		<Select.Root
			type="single"
			value={selectedStudyId ?? ''}
			onValueChange={(value) => value && void selectStudy(value)}
		>
			<Select.Trigger id="extraction-study" aria-label="Study" class="w-full"
				>{selectedStudyLabel}</Select.Trigger
			>
			<Select.Content>
				<Select.Group>
					{#each studies as study (study.id)}
						<Select.Item value={study.id} label={study.title}>{study.title}</Select.Item
						>
					{/each}
				</Select.Group>
			</Select.Content>
		</Select.Root>
	{/if}
{/snippet}

{#snippet header()}
	<header class="flex flex-wrap items-start justify-between gap-3">
		<div class="flex min-w-0 flex-col gap-1">
			<h2 class="editorial-title text-xl leading-tight">
				{view === 'fields'
					? 'Extraction fields'
					: (selectedStudy?.title ?? 'Select a study')}
			</h2>
			<p class="text-sm text-muted-foreground">
				{#if view === 'fields'}
					The data collected from every study. Changes apply project-wide.
				{:else if fields.length === 0}
					Define the fields to collect before extracting.
				{:else}
					{filledCount} of {fields.length} fields filled{#if proposalDirty}<span
							data-testid="extraction-draft-status"
						>
							· proposal edited locally</span
						>{/if}
				{/if}
			</p>
		</div>
		<div class="flex items-center gap-2">
			{#if view === 'data' && selectedStudyId && fields.length > 0 && !activeProposal}
				<Button
					variant="outline"
					size="sm"
					disabled={!canGenerate}
					onclick={() => void generateProposal()}
				>
					{#if generateMutation.isPending || reviewRun.isActive}<Spinner
							data-icon="inline-start"
						/>{:else}<Brain data-icon="inline-start" />{/if}
					{generateMutation.isPending || reviewRun.isActive
						? 'Generating…'
						: 'Generate proposal'}
				</Button>
			{/if}
			<Button
				variant="ghost"
				size="sm"
				aria-pressed={view === 'fields'}
				onclick={() => (view = view === 'fields' ? 'data' : 'fields')}
			>
				{#if view === 'fields'}
					<ArrowLeft data-icon="inline-start" />Back to data
				{:else}
					<Settings2 data-icon="inline-start" />
					Fields
					<span class="text-muted-foreground tabular-nums">{fields.length}</span>
				{/if}
			</Button>
		</div>
		{#if providerUnavailable && view === 'data'}
			<p
				class="basis-full text-sm text-muted-foreground"
				role="status"
				data-testid="extraction-provider-unavailable"
			>
				AI extraction isn't configured. Enter values directly in the sheet.
			</p>
		{/if}
	</header>
{/snippet}

{#snippet fieldsView()}
	<section class="flex min-w-0 flex-col gap-6" data-testid="extraction-schema-card">
		{#if fieldsQuery.isPending}
			<div data-testid="extraction-fields-loading"><Skeleton class="h-24 w-full" /></div>
		{:else if fields.length === 0}
			<p class="text-sm text-muted-foreground" data-testid="extraction-fields-empty">
				No extraction fields are configured. Add a field to define the study data to
				extract.
			</p>
		{:else}
			<ol class="flex flex-col" data-testid="extraction-fields">
				{#each fields as field (field.id)}
					<li
						class="flex flex-col gap-2 border-b py-2.5"
						data-testid={`extraction-field-${field.field_key}`}
					>
						{#if editingDefinitionId === field.id}
							<form
								class="flex max-w-md flex-col gap-3"
								aria-label={`Edit ${field.label}`}
								onsubmit={(event) => {
									event.preventDefault();
									void saveFieldEdit(field);
								}}
							>
								<p class="text-xs text-muted-foreground">
									Key {field.field_key} stays the same. The value type cannot change
									once the field has values.
								</p>
								<Field.Field>
									<Field.FieldLabel for={`edit-field-label-${field.id}`}
										>Label</Field.FieldLabel
									>
									<Input
										id={`edit-field-label-${field.id}`}
										bind:value={editFieldLabel}
										required
									/>
								</Field.Field>
								<Field.Field>
									<Field.FieldLabel for={`edit-field-type-${field.id}`}
										>Value type</Field.FieldLabel
									>
									<Select.Root
										type="single"
										value={editFieldType}
										onValueChange={(value) => {
											if (value && isExtractionValueType(value))
												editFieldType = value;
										}}
									>
										<Select.Trigger id={`edit-field-type-${field.id}`}
											>{editFieldType}</Select.Trigger
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
								<Field.Field orientation="horizontal">
									<Checkbox
										id={`edit-field-required-${field.id}`}
										bind:checked={editFieldRequired}
									/>
									<Field.FieldLabel
										for={`edit-field-required-${field.id}`}
										class="font-normal">Required</Field.FieldLabel
									>
								</Field.Field>
								{#if editFieldError}
									<p class="text-sm text-destructive" role="alert">
										{editFieldError}
									</p>
								{/if}
								<div class="flex gap-2">
									<Button
										type="submit"
										size="sm"
										disabled={updateFieldMutation.isPending}
									>
										{updateFieldMutation.isPending ? 'Saving…' : 'Save field'}
									</Button>
									<Button
										type="button"
										variant="ghost"
										size="sm"
										onclick={() => (editingDefinitionId = null)}>Cancel</Button
									>
								</div>
							</form>
						{:else}
							<div class="flex flex-wrap items-baseline justify-between gap-2">
								<span class="font-medium">{field.label}</span>
								<span class="text-xs text-muted-foreground"
									>{valueTypeLabel(field.value_type)} · {field.required
										? 'required'
										: 'optional'} · {field.field_key} · v{field.version}</span
								>
							</div>
							<div>
								<Button
									type="button"
									variant="outline"
									size="xs"
									aria-label={`Edit ${field.label}`}
									onclick={() => startFieldEdit(field)}
								>
									Edit
								</Button>
							</div>
						{/if}
					</li>
				{/each}
			</ol>
		{/if}
		<div class="flex max-w-md flex-col gap-3">
			<h3 class="text-sm font-semibold">Add a field</h3>
			<form
				class="flex flex-col gap-4"
				aria-label="Add extraction field"
				onsubmit={(event) => {
					event.preventDefault();
					void createField();
				}}
			>
				{#if fieldFormError}<p
						id="extraction-field-form-error"
						class="text-sm text-destructive"
						role="alert"
					>
						{fieldFormError}
					</p>{/if}
				<Field.FieldGroup>
					<Field.Field>
						<Field.FieldLabel for="extraction-field-label">Label</Field.FieldLabel>
						<Input
							id="extraction-field-label"
							bind:value={newFieldLabel}
							placeholder="Sample size"
							aria-describedby={fieldFormError
								? 'extraction-field-form-error'
								: undefined}
							aria-invalid={Boolean(fieldFormError)}
						/>
					</Field.Field>
					<div class="grid gap-4">
						<Field.Field>
							<Field.FieldLabel for="extraction-field-type"
								>Value type</Field.FieldLabel
							>
							<Select.Root
								type="single"
								value={newValueType}
								onValueChange={(value) => {
									if (value && isExtractionValueType(value)) newValueType = value;
								}}
							>
								<Select.Trigger id="extraction-field-type"
									>{newValueType}</Select.Trigger
								>
								<Select.Content>
									<Select.Group>
										{#each EXTRACTION_VALUE_TYPES as value (value)}
											<Select.Item {value} label={value}>{value}</Select.Item>
										{/each}
									</Select.Group>
								</Select.Content>
							</Select.Root>
						</Field.Field>
					</div>
					<Field.Field orientation="horizontal">
						<Checkbox id="extraction-field-required" bind:checked={newFieldRequired} />
						<Field.FieldLabel for="extraction-field-required" class="font-normal"
							>Required field</Field.FieldLabel
						>
					</Field.Field>
				</Field.FieldGroup>
				<details class="text-sm">
					<summary class="cursor-pointer text-muted-foreground hover:text-foreground"
						>Key and version · {newFieldKey || derivedFieldKey || 'derived from label'} ·
						v{newFieldVersion}</summary
					>
					<div class="flex flex-col gap-4 pt-3">
						<Field.Field>
							<Field.FieldLabel for="extraction-field-key">Field key</Field.FieldLabel
							>
							<Input
								id="extraction-field-key"
								bind:value={newFieldKey}
								placeholder={derivedFieldKey || 'sample_size'}
								aria-describedby={fieldFormError
									? 'extraction-field-form-error'
									: undefined}
								aria-invalid={Boolean(fieldFormError)}
							/>
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
								aria-describedby={fieldFormError
									? 'extraction-field-form-error'
									: undefined}
								aria-invalid={Boolean(fieldFormError)}
							/>
						</Field.Field>
					</div>
				</details>
				<Button type="submit" disabled={createFieldMutation.isPending}>
					{#if createFieldMutation.isPending}<Spinner
							data-icon="inline-start"
						/>{:else}<Plus data-icon="inline-start" />{/if}
					{createFieldMutation.isPending ? 'Adding field…' : 'Add field'}
				</Button>
			</form>
		</div>
	</section>
{/snippet}

{#snippet sheet()}
	{#if selectedStudyId}
		<section
			class="flex min-w-0 flex-col gap-2"
			data-testid="extraction-accepted-card"
			aria-label="Accepted values"
		>
			{#if valuesQuery.isPending}
				<div data-testid="extraction-values-loading"><Skeleton class="h-24 w-full" /></div>
			{:else}
				{#if toVerifyCount > 0}
					<p
						class="text-sm text-muted-foreground"
						data-testid="extraction-to-verify-count"
					>
						<Badge variant="warning" size="sm">To verify</Badge>
						{toVerifyCount}
						{toVerifyCount === 1 ? 'value was' : 'values were'} filled in by the AI. Check
						each against its source, then confirm it.
					</p>
				{/if}
				<ol class="flex flex-col" data-testid="accepted-extraction-values">
					{#each sheetRows as row (row.key)}
						<li
							class="grid gap-x-6 gap-y-1 border-b py-3 sm:grid-cols-[minmax(8rem,14rem)_minmax(0,1fr)]"
							data-testid={row.value
								? `accepted-extraction-value-${row.value.field_definition_id}`
								: undefined}
						>
							<span class="text-sm font-medium"
								>{row.label}{#if row.required}<span class="text-muted-foreground">
										*</span
									>{/if}</span
							>
							{#if row.field && editingFieldId === row.field.id}
								{@const field = row.field}
								<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
								<form
									class="flex min-w-0 flex-col gap-2"
									aria-label={`Edit ${field.label}`}
									data-testid={`extraction-edit-${field.id}`}
									onkeydown={onEditKeydown}
									onsubmit={(event) => {
										event.preventDefault();
										void saveEdit(field);
									}}
								>
									{#if field.value_type === 'boolean'}
										<Field.Field orientation="horizontal">
											<Checkbox
												id={`extraction-edit-value-${field.id}`}
												bind:checked={editBoolean}
											/>
											<Field.FieldLabel
												for={`extraction-edit-value-${field.id}`}
												class="font-normal">Yes</Field.FieldLabel
											>
										</Field.Field>
									{:else}
										<Input
											id={`extraction-edit-value-${field.id}`}
											aria-label={`${field.label} value`}
											type={field.value_type === 'number'
												? 'number'
												: field.value_type === 'date'
													? 'date'
													: 'text'}
											step={field.value_type === 'number' ? 'any' : undefined}
											value={editText}
											oninput={(event) =>
												(editText = event.currentTarget.value)}
											autofocus
											aria-invalid={Boolean(editError)}
										/>
									{/if}
									<Input
										aria-label={`${field.label} note`}
										placeholder="Note (optional)"
										bind:value={editRationale}
									/>
									<p class="text-xs text-muted-foreground">
										{#if editSource}
											Cites page {editSource.page}.
											<Button
												variant="link"
												size="xs"
												class="px-0"
												onclick={() => (editSource = undefined)}
												>Remove citation</Button
											>
										{:else}
											Optional: click a passage in Source evidence to cite it.
										{/if}
									</p>
									{#if editError}<p class="text-sm text-destructive" role="alert">
											{editError}
										</p>{/if}
									<div class="flex flex-wrap items-center gap-2">
										<Button type="submit" size="sm" disabled={savingValue}>
											{#if recordValueMutation.isPending}<Spinner
													data-icon="inline-start"
												/>{:else}<Check data-icon="inline-start" />{/if}
											Save
										</Button>
										<Button
											type="button"
											variant="ghost"
											size="sm"
											disabled={savingValue}
											onclick={cancelEdit}>Cancel</Button
										>
										{#if row.value}
											<Button
												type="button"
												variant="ghost"
												size="sm"
												disabled={savingValue}
												onclick={() => void clearEdit(field)}
												>Clear value</Button
											>
										{/if}
									</div>
								</form>
							{:else if row.value}
								{@const value = row.value}
								<div class="flex min-w-0 flex-col gap-1">
									<div class="flex min-w-0 items-baseline gap-2">
										<span class="text-sm"
											>{typedValueLabel(value.value)}
											{#if value.needs_verification}
												<Badge
													variant="warning"
													size="sm"
													class="ml-1"
													data-testid={`extraction-to-verify-${value.field_definition_id}`}
													>To verify</Badge
												>
											{:else}
												<span class="ml-1 text-xs text-success"
													>{value.verified_at
														? 'Confirmed'
														: value.source_block_id
															? 'Accepted'
															: 'Entered'}</span
												>
											{/if}</span
										>
										{#if value.needs_verification}
											<Button
												variant="outline"
												size="xs"
												disabled={confirmValueMutation.isPending}
												aria-label={`Confirm ${row.label}`}
												data-testid={`extraction-confirm-${value.field_definition_id}`}
												onclick={() => void confirmValue(value.id)}
												><Check data-icon="inline-start" />Confirm</Button
											>
										{/if}
										{#if row.field}
											<Button
												variant="ghost"
												size="icon-xs"
												aria-label={`Edit ${row.label}`}
												data-testid={`extraction-edit-button-${row.field.id}`}
												onclick={() => startEdit(row.field, value)}
											>
												<Pencil />
											</Button>
										{/if}
									</div>
									{#if value.rationale}<span class="text-xs text-muted-foreground"
											>{value.rationale}</span
										>{/if}
									{#if value.report_id && value.source_document_id && value.source_block_id && value.source_page && value.source_parser_version && value.source_content_hash}
										<a
											class="text-xs text-primary underline underline-offset-4"
											href={resolve(
												`projects/${encodeURIComponent(projectId)}/screening/full-text${buildExtractionEvidenceSearch(
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
											<CitationLabel
												{projectId}
												reportId={value.report_id}
												documentId={value.source_document_id}
												blockId={value.source_block_id}
												page={value.source_page}
												title={reportTitle(value.report_id)}
												technical={`document ${value.source_document_id} · block ${value.source_block_id}`}
											/>
										</a>
										<details class="text-xs text-muted-foreground">
											<summary class="cursor-pointer">Provenance</summary>
											<EvidenceLabel
												label="Cited from the parsed PDF"
												technical={`document ${value.source_document_id} · parser ${value.source_parser_version} · content hash ${value.source_content_hash}`}
												class="pt-1"
											/>
										</details>
									{:else}
										<span class="text-xs text-muted-foreground"
											>Entered by a reviewer, no cited source</span
										>
									{/if}
								</div>
							{:else if row.field}
								{@const field = row.field}
								<button
									type="button"
									class="w-fit rounded-md px-1 text-left text-sm text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
									aria-label={`Enter value for ${row.label}`}
									data-testid={`extraction-enter-${field.id}`}
									onclick={() => startEdit(field, undefined)}>—</button
								>
							{:else}
								<span class="text-sm text-muted-foreground">—</span>
							{/if}
						</li>
					{:else}
						<li
							class="py-3 text-sm text-muted-foreground"
							data-testid="extraction-values-empty"
						>
							No fields yet.
							<Button
								variant="link"
								size="xs"
								class="px-0"
								onclick={() => (view = 'fields')}>Add extraction fields</Button
							>
						</li>
					{/each}
				</ol>
			{/if}
		</section>
	{/if}
{/snippet}

{#snippet proposalSection()}
	{#if selectedStudyId && (activeProposal || loading)}
		<section class="flex min-w-0 flex-col gap-4" data-testid="extraction-review-card">
			<h3 class="text-sm font-semibold">Proposal to review</h3>
			<div class="flex min-w-0 flex-col gap-4">
				{#if !selectedStudyId}
					<div data-testid="extraction-review-empty">
						<StatePanel
							state="empty"
							title="Select a study"
							description="Choose a study above to see its proposed and saved values."
							class="min-h-0 rounded-md border-0 p-4"
						/>
					</div>
				{:else if loading}
					<div data-testid="extraction-review-loading">
						<StatePanel
							state="loading"
							title="Loading review"
							description="Retrieving proposal, accepted values, and provenance."
							class="min-h-0 rounded-md border-0 p-4"
						/>
					</div>
				{:else if !activeProposal}
					<div data-testid="extraction-review-empty">
						<StatePanel
							state="empty"
							title="No pending proposal"
							description="Generate a proposal, then check each suggested value against its source."
							class="min-h-0 rounded-md border-0 p-4"
						/>
					</div>
				{:else}
					{@const proposal = activeProposal}
					<div class="flex flex-wrap items-center gap-2">
						<Badge variant="secondary">pending</Badge>
						{#if proposalDirty}<Badge variant="outline">Edited locally</Badge>{/if}
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
							<div data-testid={`extraction-proposal-field-${draft.field_id}`}>
								<Surface as="article" tone="plain" class="border-t py-4">
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
											>{:else}<Badge variant="secondary"
												>{draft.value.kind}</Badge
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
															setDraftTextValue(
																draft.field_id,
																event
															)}
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
															setDraftNumberValue(
																draft.field_id,
																event
															)}
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
															setDraftDateValue(
																draft.field_id,
																event
															)}
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
													Required field: insufficiency cannot be
													accepted. Enter a source-backed value or
													generate a new grounded proposal.
												</p>
											{/if}
										{/if}
									</Field.FieldGroup>

									{#if draft.kind === 'value'}
										<Surface
											as="aside"
											tone="subtle"
											class="mt-4 p-3 text-sm"
											label="Evidence provenance"
										>
											<div class="flex items-center gap-2 font-medium">
												<FileSearch aria-hidden="true" />Evidence
											</div>
											<a
												class="mt-2 block text-primary underline underline-offset-4"
												data-testid="extraction-evidence-link"
												href={resolve(
													`projects/${encodeURIComponent(projectId)}/screening/full-text${buildExtractionEvidenceSearch(draft.source)}`
												)}
											>
												<CitationLabel
													{projectId}
													reportId={draft.source.report_id}
													documentId={draft.source.document_id}
													blockId={draft.source.document_block_id}
													page={draft.source.page}
													title={reportTitle(draft.source.report_id)}
													technical={`document ${draft.source.document_id} · block ${draft.source.document_block_id}`}
												/>
											</a>
											<EvidenceLabel
												label="Cited from the parsed PDF"
												technical={`document ${draft.source.document_id} · parser ${draft.source.parser_version} · content hash ${draft.source.content_hash}`}
												class="mt-1 text-xs text-muted-foreground"
											/>
										</Surface>
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
											<RefreshCw data-icon="inline-start" />Restore proposed
											value
										</Button>
									{:else}
										<p class="mt-3 text-sm text-muted-foreground" role="status">
											No source block is attached to this
											insufficient-evidence proposal. Entering a value is
											unavailable; generate a new grounded proposal before
											accepting a value.
										</p>
									{/if}
								</Surface>
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
			</div>
		</section>
	{/if}
{/snippet}

{#snippet center()}
	{#if queryError}
		<Alert.Root
			variant="destructive"
			role="alert"
			data-testid="extraction-query-error"
			data-extraction-state="error"
		>
			<Alert.Title>Extraction data unavailable</Alert.Title>
			<Alert.Description>{queryError}</Alert.Description>
		</Alert.Root>
	{/if}
	{#if actionStatus === 'validation' && actionError}
		<Alert.Root
			role="alert"
			data-testid="extraction-action-status"
			data-action-status="validation"
		>
			<Alert.Title>Review needs attention</Alert.Title>
			<Alert.Description>{actionError}</Alert.Description>
		</Alert.Root>
	{/if}
	{@render header()}
	{#if view === 'fields'}
		{@render fieldsView()}
	{:else}
		{@render proposalSection()}
		{@render sheet()}
	{/if}
{/snippet}

{#snippet source()}
	<aside class="flex min-w-0 flex-col gap-3" aria-label="Extraction source evidence">
		<h3 class="text-sm font-semibold">Source evidence</h3>
		{#if studyReports.length}
			<label for="extraction-source-report" class="mt-3 block text-xs text-muted-foreground"
				>Study report</label
			>
			<select
				id="extraction-source-report"
				class="my-2 h-10 w-full rounded-md border bg-background px-3 text-sm"
				value={sourceReportId}
				onchange={(event) => (evidenceReportId = event.currentTarget.value)}
			>
				{#each studyReports as report (report.report_id)}
					<option value={report.report_id}>{report.title ?? report.report_id}</option>
				{/each}
			</select>
		{/if}
		{#if !sourceReportId}
			<p class="mt-3 text-sm text-muted-foreground">
				Assign a report to this study to read source evidence.
			</p>
		{:else if sourceDocuments.isPending || (sourceBlocks.isPending && sourceDocumentId)}
			<p class="text-sm text-muted-foreground">Loading source evidence…</p>
		{:else if sourceBlocks.error || sourceDocuments.error}
			<Alert.Root variant="destructive"
				><Alert.Title>Source unavailable</Alert.Title><Alert.Description
					>{sourceBlocks.error?.message ??
						sourceDocuments.error?.message}</Alert.Description
				></Alert.Root
			>
		{:else}
			<div class="flex flex-col gap-4">
				{#each sourceBlocks.data?.data ?? [] as block (block.id)}
					<article class="border-b pb-4">
						<a
							class="text-xs text-primary underline"
							href={resolve(
								`projects/${encodeURIComponent(projectId)}/screening/full-text${fullTextUrlString(
									{
										filter: 'all',
										report: sourceReportId,
										block: block.id,
										page: block.page_number
									}
								)}`
							)}>Page {block.page_number}</a
						>

						{#if editingFieldId}
							<button
								type="button"
								class={[
									'mt-2 block w-full rounded-md p-1.5 text-left text-sm leading-6 whitespace-pre-wrap transition-colors hover:bg-muted focus-visible:outline-2 focus-visible:outline-ring',
									editSource?.blockId === block.id &&
										'bg-accent shadow-inset-accent'
								]}
								aria-pressed={editSource?.blockId === block.id}
								data-testid="extraction-cite-block"
								onclick={() => citeBlock(block)}
							>
								{block.text}
							</button>
						{:else}
							<p class="mt-2 text-sm leading-6 whitespace-pre-wrap">
								{block.text}
							</p>
						{/if}
					</article>
				{:else}
					<p class="mt-3 text-sm text-muted-foreground">
						No parsed source blocks available. Add the report's full text before
						checking source-backed values.
					</p>
				{/each}
			</div>
		{/if}
	</aside>
{/snippet}

{#if wide.current}
	<div class="h-full min-h-0" data-testid="extraction-page" data-extraction-state={pageState}>
		<Resizable.PaneGroup
			direction="horizontal"
			class="h-full"
			autoSaveId="deepref:extraction-layout"
		>
			<Resizable.Pane order={1} defaultSize={20} minSize={14} maxSize={34}>
				<nav class="flex h-full min-h-0 flex-col" aria-label="Studies">
					{@render studyList()}
				</nav>
			</Resizable.Pane>
			<Resizable.Handle />
			<Resizable.Pane order={2} defaultSize={50} minSize={34}>
				<div class="flex h-full flex-col gap-6 overflow-y-auto px-8 py-6">
					{@render center()}
				</div>
			</Resizable.Pane>
			<Resizable.Handle />
			<Resizable.Pane order={3} defaultSize={30} minSize={18} maxSize={45}>
				<div class="h-full overflow-y-auto px-5 py-6">{@render source()}</div>
			</Resizable.Pane>
		</Resizable.PaneGroup>
	</div>
{:else}
	<PageTemplate testId="extraction-page" data-extraction-state={pageState} containerClass="gap-6">
		{@render mobileStudyPicker()}
		{@render center()}
		{#if view === 'data'}
			<details class="disclosure">
				<summary>Source evidence</summary>
				<div class="pt-3">{@render source()}</div>
			</details>
		{/if}
	</PageTemplate>
{/if}
