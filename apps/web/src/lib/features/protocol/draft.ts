import type { ProtocolDto, SaveProtocolRequest } from './api';
import {
	REQUIRED_FRAMEWORK_FIELDS,
	duplicateCustomKeys,
	frameworkFieldsForKind,
	humanizeKey,
	isCriterionDimension,
	isCriterionKind,
	isCriterionStage,
	isFrameworkKind,
	type CriterionDimension,
	type CriterionKind,
	type CriterionStage,
	type FrameworkKind
} from './codecs';

export type DraftClientIdKind = 'criterion' | 'field';
export type DraftClientIdFactory = (kind: DraftClientIdKind) => string;

export type DraftCriterion = {
	clientId: string;
	id?: string;
	kind: CriterionKind;
	stage: CriterionStage;
	dimension: CriterionDimension;
	label: string;
	description: string;
};

export type CustomFrameworkField = {
	clientId: string;
	key: string;
	value: string;
};

export type ProtocolDraft = {
	id?: string;
	version: number;
	status: 'draft' | 'published' | 'superseded';
	name: string;
	objective: string;
	question: string;
	frameworkKind: FrameworkKind;
	frameworkFields: Record<string, string>;
	customFrameworkFields: CustomFrameworkField[];
	frameworkFieldSnapshots: Partial<Record<FrameworkKind, Record<string, string>>>;
	customFrameworkSnapshot?: CustomFrameworkField[];
	criteria: DraftCriterion[];
	revision: number;
	amendmentOf?: string | null;
};

type UnknownRecord = Record<string, unknown>;

export function emptyProtocolDraft(): ProtocolDraft {
	return {
		version: 1,
		status: 'draft',
		name: '',
		objective: '',
		question: '',
		frameworkKind: 'pico',
		frameworkFields: frameworkFieldsForKind('pico', {}),
		customFrameworkFields: [],
		frameworkFieldSnapshots: {},
		criteria: [],
		revision: 0
	};
}

export function protocolDraftFromDto(
	value: ProtocolDto,
	nextClientId: DraftClientIdFactory
): ProtocolDraft {
	const kind = isFrameworkKind(value.framework_kind) ? value.framework_kind : 'custom';
	const frameworkFields = stringRecord(value.framework_fields);
	const knownFields = frameworkFieldsForKind(kind, frameworkFields);
	const customFrameworkFields: CustomFrameworkField[] =
		kind === 'custom' ? customFieldsFromRecord(frameworkFields, nextClientId) : [];
	return {
		id: value.id,
		version: value.version,
		status: normalizeStatus(value.status),
		name: value.name,
		objective: value.objective,
		question: value.question,
		frameworkKind: kind,
		frameworkFields: knownFields,
		customFrameworkFields,
		frameworkFieldSnapshots: kind === 'custom' ? {} : { [kind]: { ...knownFields } },
		customFrameworkSnapshot:
			kind === 'custom' ? cloneCustomFields(customFrameworkFields) : undefined,
		criteria: parseCriteria(value.criteria, nextClientId),
		revision: value.revision,
		amendmentOf: value.amendment_of
	};
}

export function customFieldsFromRecord(
	values: Readonly<Record<string, string>>,
	nextClientId: DraftClientIdFactory
): CustomFrameworkField[] {
	return Object.entries(values).map(([key, value]) => ({
		clientId: nextClientId('field'),
		key,
		value
	}));
}

export function cloneCustomFields(
	fields: ReadonlyArray<CustomFrameworkField>
): CustomFrameworkField[] {
	return fields.map((field) => ({ ...field }));
}

export function buildSaveProtocolRequest(value: ProtocolDraft): SaveProtocolRequest {
	return {
		name: value.name.trim(),
		objective: value.objective.trim(),
		question: value.question.trim(),
		framework: { kind: value.frameworkKind, fields: frameworkPayload(value) },
		criteria: value.criteria.map((criterion) => ({
			...(criterion.id ? { id: criterion.id } : {}),
			kind: criterion.kind,
			stage: criterion.stage,
			dimension: criterion.dimension,
			label: criterion.label.trim(),
			description: criterion.description.trim()
		})),
		protocol_version_id: value.id,
		expected_revision: value.revision
	};
}

export function validateProtocolDraft(value: ProtocolDraft): string[] {
	return [
		...requiredProtocolErrors(value),
		...frameworkValidationErrors(value),
		...criteriaValidationErrors(value.criteria)
	];
}

function requiredProtocolErrors(value: ProtocolDraft): string[] {
	const errors: string[] = [];
	if (!value.name.trim()) errors.push('Give the protocol a name.');
	if (!value.objective.trim()) errors.push('Add the review objective.');
	if (!value.question.trim()) errors.push('Add the research question.');
	return errors;
}

function frameworkValidationErrors(value: ProtocolDraft): string[] {
	if (value.frameworkKind === 'custom') {
		return customFrameworkErrors(value.customFrameworkFields);
	}
	return knownFrameworkErrors(value.frameworkKind, value.frameworkFields);
}

function customFrameworkErrors(fields: ReadonlyArray<CustomFrameworkField>): string[] {
	const errors: string[] = [];
	const duplicateKeys = duplicateCustomKeys(fields);
	if (duplicateKeys.length > 0) {
		errors.push(`Custom framework field names must be unique: ${duplicateKeys.join(', ')}.`);
	}
	if (fields.some((field) => !field.key.trim() || !field.value.trim())) {
		errors.push('Complete or remove every custom framework field.');
	}
	return errors;
}

function knownFrameworkErrors(
	kind: Exclude<FrameworkKind, 'custom'>,
	fields: Readonly<Record<string, string>>
): string[] {
	return REQUIRED_FRAMEWORK_FIELDS[kind].flatMap((field) =>
		fields[field]?.trim()
			? []
			: [`Complete the required ${humanizeKey(field)} framework field.`]
	);
}

function criteriaValidationErrors(criteria: ReadonlyArray<DraftCriterion>): string[] {
	const hasIncompleteCriterion = criteria.some(
		(criterion) => !criterion.label.trim() || !criterion.description.trim()
	);
	return hasIncompleteCriterion ? ['Complete every eligibility criterion or remove it.'] : [];
}

function frameworkPayload(value: ProtocolDraft): Record<string, string> {
	if (value.frameworkKind !== 'custom') {
		return frameworkFieldsForKind(value.frameworkKind, value.frameworkFields);
	}
	const fields: Record<string, string> = {};
	for (const field of value.customFrameworkFields) {
		const key = field.key.trim();
		if (key) fields[key] = field.value.trim();
	}
	return fields;
}

function stringRecord(value: unknown): Record<string, string> {
	if (!isRecord(value)) return {};
	return Object.fromEntries(
		Object.entries(value).filter(
			(entry): entry is [string, string] => typeof entry[1] === 'string'
		)
	);
}

function parseCriteria(value: unknown, nextClientId: DraftClientIdFactory): DraftCriterion[] {
	if (!Array.isArray(value)) return [];
	const criteria: DraftCriterion[] = [];
	for (const item of value) {
		const criterion = parseCriterion(item, nextClientId);
		if (criterion) criteria.push(criterion);
	}
	return criteria;
}

function parseCriterion(
	value: unknown,
	nextClientId: DraftClientIdFactory
): DraftCriterion | undefined {
	if (!isRecord(value)) return undefined;
	const kind = criterionKind(value.kind);
	const stage = criterionStage(value.stage);
	const dimension = criterionDimension(value.dimension);
	if (!kind || !stage || !dimension) return undefined;
	const id = stringValue(value.id);
	return {
		clientId: id ?? nextClientId('criterion'),
		id,
		kind,
		stage,
		dimension,
		label: stringValue(value.label) ?? '',
		description: stringValue(value.description) ?? ''
	};
}

function criterionKind(value: unknown): CriterionKind | undefined {
	return typeof value === 'string' && isCriterionKind(value) ? value : undefined;
}

function criterionStage(value: unknown): CriterionStage | undefined {
	return typeof value === 'string' && isCriterionStage(value) ? value : undefined;
}

function criterionDimension(value: unknown): CriterionDimension | undefined {
	return typeof value === 'string' && isCriterionDimension(value) ? value : undefined;
}

function stringValue(value: unknown): string | undefined {
	return typeof value === 'string' ? value : undefined;
}

function isRecord(value: unknown): value is UnknownRecord {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function normalizeStatus(value: string): ProtocolDraft['status'] {
	if (value === 'published' || value === 'superseded') return value;
	return 'draft';
}
