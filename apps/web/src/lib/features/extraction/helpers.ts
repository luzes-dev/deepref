import type {
	AiExtractedFieldDto,
	AiExtractionEvidenceDto,
	AiTypedExtractionValueDto,
	ExtractionFieldDto
} from '$lib/api/generated/models';

export const EXTRACTION_VALUE_TYPES = ['text', 'number', 'boolean', 'date'] as const;

export type ExtractionValueType = (typeof EXTRACTION_VALUE_TYPES)[number];

export type ExtractionDraftTypedValue =
	| { kind: 'text'; value: string }
	| { kind: 'number'; value: string }
	| { kind: 'boolean'; value: boolean }
	| { kind: 'date'; value: string };

export type ExtractionDraftField =
	| {
			field_id: string;
			field_version: number;
			kind: 'value';
			rationale: string;
			source: AiExtractionEvidenceDto;
			value: ExtractionDraftTypedValue;
	  }
	| {
			field_id: string;
			field_version: number;
			kind: 'insufficient_evidence';
			rationale: string;
	  };

export type ExtractionSerializationResult =
	{ ok: true; fields: AiExtractedFieldDto[] } | { ok: false; message: string };

export function isExtractionValueType(value: string): value is ExtractionValueType {
	return EXTRACTION_VALUE_TYPES.some((type) => type === value);
}

function isIsoDate(value: string): boolean {
	if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) return false;
	const parsed = new Date(`${value}T00:00:00.000Z`);
	return !Number.isNaN(parsed.valueOf()) && parsed.toISOString().slice(0, 10) === value;
}

export function draftFromAiField(field: AiExtractedFieldDto): ExtractionDraftField {
	switch (field.kind) {
		case 'insufficient_evidence':
			return { ...field };
		case 'value':
			return {
				field_id: field.field_id,
				field_version: field.field_version,
				kind: 'value',
				rationale: field.rationale,
				source: field.source,
				value: draftValueFromAiValue(field.value)
			};
		default: {
			const exhaustive: never = field;
			return exhaustive;
		}
	}
}

function draftValueFromAiValue(value: AiTypedExtractionValueDto): ExtractionDraftTypedValue {
	switch (value.kind) {
		case 'text':
			return { kind: 'text', value: value.value };
		case 'number':
			return { kind: 'number', value: String(value.value) };
		case 'boolean':
			return { kind: 'boolean', value: value.value };
		case 'date':
			return { kind: 'date', value: value.value };
		default: {
			const exhaustive: never = value;
			return exhaustive;
		}
	}
}

export function validateExtractionDrafts(
	fields: ExtractionFieldDto[],
	drafts: ExtractionDraftField[]
): string | undefined {
	if (drafts.length !== fields.length) {
		return 'The proposal does not contain one reviewed value for every current extraction field.';
	}

	const seenFieldIds = new Set<string>();
	for (const draft of drafts) {
		const duplicateError = markFieldAsSeen(seenFieldIds, draft.field_id);
		if (duplicateError) return duplicateError;

		const field = fields.find((candidate) => candidate.id === draft.field_id);
		if (!field) return `Extraction field ${draft.field_id} is no longer current.`;

		const validationError = validateDraftForField(field, draft);
		if (validationError) return validationError;
	}

	return findMissingFieldError(fields, seenFieldIds);
}

function markFieldAsSeen(seenFieldIds: Set<string>, fieldId: string): string | undefined {
	if (seenFieldIds.has(fieldId)) {
		return `Field ${fieldId} appears more than once in the reviewed proposal.`;
	}
	seenFieldIds.add(fieldId);
	return undefined;
}

function validateDraftForField(
	field: ExtractionFieldDto,
	draft: ExtractionDraftField
): string | undefined {
	if (field.version !== draft.field_version) {
		return `${field.label} changed to version ${field.version}. Refresh the proposal before accepting it.`;
	}
	if (!isExtractionValueType(field.value_type)) {
		return `${field.label} has unsupported value type ${field.value_type}.`;
	}

	return draft.kind === 'insufficient_evidence'
		? validateInsufficientEvidence(field, draft)
		: validateValueDraft(field, draft);
}

function validateInsufficientEvidence(
	field: ExtractionFieldDto,
	draft: Extract<ExtractionDraftField, { kind: 'insufficient_evidence' }>
): string | undefined {
	if (field.required) return `Required field “${field.label}” cannot be marked insufficient.`;
	return draft.rationale.trim()
		? undefined
		: `Add a rationale for insufficient evidence in “${field.label}”.`;
}

function validateValueDraft(
	field: ExtractionFieldDto,
	draft: Extract<ExtractionDraftField, { kind: 'value' }>
): string | undefined {
	if (!draft.rationale.trim()) return `Add a rationale for “${field.label}”.`;
	if (draft.value.kind !== field.value_type) {
		return `${field.label} expects ${field.value_type}, not ${draft.value.kind}.`;
	}

	return validateTypedValue(field.label, draft.value);
}

function validateTypedValue(label: string, value: ExtractionDraftTypedValue): string | undefined {
	switch (value.kind) {
		case 'text':
			return value.value.trim() ? undefined : `${label} cannot be blank.`;
		case 'number':
			return value.value.trim() && Number.isFinite(Number(value.value))
				? undefined
				: `${label} must be a finite number.`;
		case 'boolean':
			return undefined;
		case 'date':
			return isIsoDate(value.value)
				? undefined
				: `${label} must use an ISO date such as 2026-08-27.`;
		default: {
			const exhaustive: never = value;
			return exhaustive;
		}
	}
}

function findMissingFieldError(
	fields: ExtractionFieldDto[],
	seenFieldIds: Set<string>
): string | undefined {
	for (const field of fields) {
		if (!seenFieldIds.has(field.id)) {
			return `Field “${field.label}” is missing from the proposal.`;
		}
	}
	return undefined;
}

export function serializeExtractionDrafts(
	drafts: ExtractionDraftField[]
): ExtractionSerializationResult {
	const fields: AiExtractedFieldDto[] = [];
	for (const draft of drafts) {
		if (draft.kind === 'insufficient_evidence') {
			fields.push({
				field_id: draft.field_id,
				field_version: draft.field_version,
				kind: 'insufficient_evidence',
				rationale: draft.rationale
			});
			continue;
		}

		const value = serializeDraftValue(draft.value);
		if (!value.ok) return value;
		fields.push({
			field_id: draft.field_id,
			field_version: draft.field_version,
			kind: 'value',
			rationale: draft.rationale,
			source: draft.source,
			value: value.value
		});
	}
	return { ok: true, fields };
}

function serializeDraftValue(
	value: ExtractionDraftTypedValue
):
	| { ok: true; value: Extract<AiExtractedFieldDto, { kind: 'value' }>['value'] }
	| { ok: false; message: string } {
	switch (value.kind) {
		case 'text':
			return { ok: true, value };
		case 'number': {
			const number = Number(value.value);
			return Number.isFinite(number)
				? { ok: true, value: { kind: 'number', value: number } }
				: { ok: false, message: 'A numeric extraction value must be finite.' };
		}
		case 'boolean':
			return { ok: true, value };
		case 'date':
			return isIsoDate(value.value)
				? { ok: true, value }
				: { ok: false, message: 'A date extraction value must be an ISO date.' };
		default: {
			const exhaustive: never = value;
			return exhaustive;
		}
	}
}

export function buildExtractionEvidenceLink(
	projectId: string,
	evidence: AiExtractionEvidenceDto
): string {
	return `/projects/${encodeURIComponent(projectId)}/screening/full-text${buildExtractionEvidenceSearch(evidence)}`;
}

export function buildExtractionEvidenceSearch(evidence: AiExtractionEvidenceDto): string {
	const search = new URLSearchParams({
		report: evidence.report_id,
		page: String(evidence.page),
		block: evidence.document_block_id
	});
	return `?${search.toString()}`;
}
