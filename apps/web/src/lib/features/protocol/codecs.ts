export const FRAMEWORK_KINDS = ['pico', 'picos', 'peco', 'peo', 'pcc', 'spider', 'custom'] as const;
export type FrameworkKind = (typeof FRAMEWORK_KINDS)[number];

export const FRAMEWORK_FIELDS = {
	pico: ['population', 'intervention', 'comparator', 'outcome'],
	picos: ['population', 'intervention', 'comparator', 'outcome', 'study_design'],
	peco: ['population', 'exposure', 'comparator', 'outcome'],
	peo: ['population', 'exposure', 'outcome'],
	pcc: ['population', 'concept', 'context'],
	spider: ['sample', 'phenomenon', 'design', 'evaluation', 'research_type'],
	custom: []
} as const satisfies Record<FrameworkKind, readonly string[]>;

export const REQUIRED_FRAMEWORK_FIELDS = {
	pico: ['population', 'intervention', 'outcome'],
	picos: ['population', 'intervention', 'outcome'],
	peco: ['population', 'exposure', 'outcome'],
	peo: ['population', 'exposure', 'outcome'],
	pcc: ['population', 'concept', 'context'],
	spider: ['sample', 'phenomenon', 'design', 'evaluation', 'research_type'],
	custom: []
} as const satisfies Record<FrameworkKind, readonly string[]>;

export type CriterionKind = 'inclusion' | 'exclusion';
export type CriterionStage = 'title_abstract' | 'full_text' | 'both';
export type CriterionDimension =
	| 'population'
	| 'intervention'
	| 'comparator'
	| 'outcome'
	| 'design'
	| 'setting'
	| 'language'
	| 'date'
	| 'other';

export const CRITERION_KINDS = ['inclusion', 'exclusion'] as const;
export const CRITERION_STAGES = ['title_abstract', 'full_text', 'both'] as const;
export const CRITERION_DIMENSIONS = [
	'population',
	'intervention',
	'comparator',
	'outcome',
	'design',
	'setting',
	'language',
	'date',
	'other'
] as const;

export function isFrameworkKind(value: string): value is FrameworkKind {
	return FRAMEWORK_KINDS.some((kind) => kind === value);
}

export function isCriterionDimension(value: string): value is CriterionDimension {
	return CRITERION_DIMENSIONS.some((dimension) => dimension === value);
}

export function isCriterionKind(value: string): value is CriterionKind {
	return CRITERION_KINDS.some((kind) => kind === value);
}

export function isCriterionStage(value: string): value is CriterionStage {
	return CRITERION_STAGES.some((stage) => stage === value);
}

export function isRequiredFrameworkField(kind: FrameworkKind, field: string): boolean {
	return REQUIRED_FRAMEWORK_FIELDS[kind].some((required) => required === field);
}

export function frameworkFieldsForKind(
	kind: FrameworkKind,
	values: Readonly<Record<string, string>>
): Record<string, string> {
	const allowed = FRAMEWORK_FIELDS[kind];
	if (kind === 'custom') return { ...values };
	return Object.fromEntries(allowed.map((field) => [field, values[field] ?? '']));
}

export function duplicateCustomKeys(fields: ReadonlyArray<{ key: string }>): string[] {
	const seen = new Set<string>();
	const duplicates = new Set<string>();
	for (const field of fields) {
		const key = field.key.trim();
		if (!key) continue;
		if (seen.has(key)) duplicates.add(key);
		seen.add(key);
	}
	return [...duplicates];
}

export function humanizeKey(value: string): string {
	return value
		.split('_')
		.map((part) => part.charAt(0).toUpperCase() + part.slice(1))
		.join(' ');
}
