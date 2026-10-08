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

/**
 * A protocol for a project that has none yet starts from the project's own name and
 * description, so the research question typed when the project was created is not retyped.
 */
export function newProtocolDraft(project?: {
	name: string;
	description?: string | null;
}): ProtocolDraft {
	const draft = emptyProtocolDraft();
	if (project) {
		draft.name = project.name.trim();
		draft.question = project.description?.trim() ?? '';
	}
	return draft;
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
		// Optional fields left blank are not sent: the server rejects blank framework values.
		return Object.fromEntries(
			Object.entries(
				frameworkFieldsForKind(value.frameworkKind, value.frameworkFields)
			).filter(([, field]) => field.trim() !== '')
		);
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

export type ProtocolTab = 'question' | 'framework' | 'criteria';

export type ProtocolIssue = {
	message: string;
	tab: ProtocolTab;
	/** DOM id of the first field that needs attention, when there is one. */
	targetId?: string;
};

/** Compact list of what still blocks publishing, each pointing at a tab and field. */
export function protocolIssues(value: ProtocolDraft): ProtocolIssue[] {
	const issues: ProtocolIssue[] = [];
	if (!value.name.trim())
		issues.push({ message: 'Name missing', tab: 'question', targetId: 'protocol-name' });
	if (!value.objective.trim())
		issues.push({
			message: 'Objective missing',
			tab: 'question',
			targetId: 'protocol-objective'
		});
	if (!value.question.trim())
		issues.push({
			message: 'Question missing',
			tab: 'question',
			targetId: 'protocol-question'
		});
	issues.push(...frameworkIssues(value));
	issues.push(...criteriaIssues(value.criteria));
	return issues;
}

function plural(count: number, singular: string, pluralForm: string): string {
	return `${count} ${count === 1 ? singular : pluralForm}`;
}

function frameworkIssues(value: ProtocolDraft): ProtocolIssue[] {
	if (value.frameworkKind === 'custom') {
		const duplicates = duplicateCustomKeys(value.customFrameworkFields);
		const incomplete = value.customFrameworkFields.filter(
			(field) => !field.key.trim() || !field.value.trim()
		).length;
		const issues: ProtocolIssue[] = [];
		if (incomplete > 0)
			issues.push({
				message: `${plural(incomplete, 'framework field', 'framework fields')} incomplete`,
				tab: 'framework'
			});
		if (duplicates.length > 0)
			issues.push({ message: 'Duplicate framework field names', tab: 'framework' });
		return issues;
	}
	const missing = REQUIRED_FRAMEWORK_FIELDS[value.frameworkKind].filter(
		(field) => !value.frameworkFields[field]?.trim()
	);
	if (missing.length === 0) return [];
	return [
		{
			message:
				missing.length === 1
					? `${humanizeKey(missing[0])} missing`
					: `${missing.length} framework fields missing`,
			tab: 'framework',
			targetId: `framework-field-${missing[0]}`
		}
	];
}

function criteriaIssues(criteria: ReadonlyArray<DraftCriterion>): ProtocolIssue[] {
	const issues: ProtocolIssue[] = [];
	// Publishing is final, and screening decisions are judged against the inclusion criteria.
	// Saving a draft stays allowed without them, so this only appears in the publish checklist.
	if (!criteria.some((criterion) => criterion.kind === 'inclusion'))
		issues.push({
			message: 'No inclusion criterion',
			tab: 'criteria',
			targetId: 'protocol-add-criterion'
		});
	const noLabel = criteria.filter((criterion) => !criterion.label.trim());
	const noDescription = criteria.filter((criterion) => !criterion.description.trim());
	if (noLabel.length > 0)
		issues.push({
			message: `${plural(noLabel.length, 'criterion', 'criteria')} without label`,
			tab: 'criteria',
			targetId: `criterion-label-${noLabel[0].clientId}`
		});
	if (noDescription.length > 0)
		issues.push({
			message: `${plural(noDescription.length, 'criterion', 'criteria')} without description`,
			tab: 'criteria',
			targetId: `criterion-description-${noDescription[0].clientId}`
		});
	return issues;
}

/** Per-project, per-revision key for the in-progress form kept in localStorage. */
export function protocolDraftStorageKey(projectId: string, value: ProtocolDraft): string {
	return `deepref:protocol-draft:${projectId}:${value.id ?? 'new'}:${value.revision}`;
}

type StoredProtocolDraft = { amending: boolean; draft: ProtocolDraft };

export function serializeStoredDraft(draft: ProtocolDraft, amending: boolean): string {
	return JSON.stringify({ amending, values: draft });
}

/**
 * Rebuilds an in-progress form kept in localStorage on top of the server draft it was
 * started from. Returns undefined when nothing meaningful differs from `base`.
 */
export function restoreStoredDraft(
	raw: string,
	base: ProtocolDraft,
	nextClientId: DraftClientIdFactory
): StoredProtocolDraft | undefined {
	let parsed: unknown;
	try {
		parsed = JSON.parse(raw);
	} catch {
		return undefined;
	}
	if (!isRecord(parsed) || !isRecord(parsed.values)) return undefined;
	const stored = parsed.values;
	const kind = stringValue(stored.frameworkKind);
	if (
		stringValue(stored.name) === undefined ||
		stringValue(stored.objective) === undefined ||
		stringValue(stored.question) === undefined ||
		!kind ||
		!isFrameworkKind(kind)
	)
		return undefined;
	const customFields: CustomFrameworkField[] = Array.isArray(stored.customFrameworkFields)
		? stored.customFrameworkFields.filter(isRecord).map((field) => ({
				clientId: nextClientId('field'),
				key: stringValue(field.key) ?? '',
				value: stringValue(field.value) ?? ''
			}))
		: [];
	const draft: ProtocolDraft = {
		...base,
		name: stringValue(stored.name) ?? '',
		objective: stringValue(stored.objective) ?? '',
		question: stringValue(stored.question) ?? '',
		frameworkKind: kind,
		frameworkFields: frameworkFieldsForKind(kind, stringRecord(stored.frameworkFields)),
		customFrameworkFields: kind === 'custom' ? customFields : [],
		frameworkFieldSnapshots: {},
		customFrameworkSnapshot: undefined,
		criteria: parseCriteria(stored.criteria, nextClientId)
	};
	if (
		JSON.stringify(buildSaveProtocolRequest(draft)) ===
		JSON.stringify(buildSaveProtocolRequest(base))
	)
		return undefined;
	return { amending: parsed.amending === true, draft };
}
