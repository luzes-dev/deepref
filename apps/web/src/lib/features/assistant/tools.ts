import { AssistantToolKind } from '$lib/api/generated/models';
import type { AssistantToolDescriptor, AssistantToolRequest } from '$lib/api/generated/models';

export type ToolName = AssistantToolRequest['tool'];
export type ToolKind = (typeof AssistantToolKind)[keyof typeof AssistantToolKind];
export type ReviewDestination =
	'screening' | 'deduplication' | 'studies' | 'extraction' | 'appraisal';

type UuidField = {
	kind: 'uuid';
	key: 'report_id' | 'document_id' | 'source_record_id' | 'candidate_report_id' | 'study_id';
	label: string;
	help: string;
};

type TextField = {
	kind: 'text';
	key: 'query' | 'definition_id';
	label: string;
	help: string;
	maxLength: number;
};

type IntegerField = {
	kind: 'integer';
	key: 'limit' | 'definition_version';
	label: string;
	help: string;
	min: number;
	max: number;
};

type BlockListField = {
	kind: 'uuid-list';
	key: 'block_ids';
	label: string;
	help: string;
};

type StageField = {
	kind: 'stage';
	key: 'stage';
	label: string;
	help: string;
};

export type ToolField = UuidField | TextField | IntegerField | BlockListField | StageField;

export type ToolMetadata = {
	name: ToolName;
	kind: ToolKind;
	label: string;
	description: string;
	fields: readonly ToolField[];
	reviewDestination: ReviewDestination | null;
};

type FieldKey = ToolField['key'];
type ParsedValue = string | number | string[];
type ParsedToolValues = Partial<Record<FieldKey, ParsedValue>>;
type FieldValidation = { kind: 'valid'; value: ParsedValue } | { kind: 'invalid'; message: string };
type RequestBuilder = (
	projectId: string,
	values: Readonly<ParsedToolValues>
) => AssistantToolRequest | null;
type ToolDefinition = ToolMetadata & {
	defaults: ToolValues;
	buildRequest: RequestBuilder;
};
type ScreeningDecisionArgs = Extract<
	AssistantToolRequest,
	{ tool: 'propose_screening_decision' }
>['args'];

export type ToolValues = Record<string, string>;

export type ToolValidation =
	| { kind: 'valid'; request: AssistantToolRequest }
	| { kind: 'invalid'; errors: Readonly<Record<string, string>> };

export type SupportedCatalogEntry = {
	metadata: ToolMetadata;
	descriptor: AssistantToolDescriptor;
};

export const ASSISTANT_TOOL_NAMES = [
	'get_project_protocol',
	'get_report',
	'read_document_blocks',
	'search_document',
	'search_project_reports',
	'get_screening_state',
	'get_study',
	'get_appraisal',
	'propose_screening_decision',
	'propose_duplicate_merge',
	'propose_study_grouping',
	'propose_classification',
	'propose_extraction',
	'propose_appraisal_answer'
] satisfies readonly ToolName[];

const uuid = (key: UuidField['key'], label: string, help: string): UuidField => ({
	kind: 'uuid',
	key,
	label,
	help
});

const text = (
	key: TextField['key'],
	label: string,
	help: string,
	maxLength: number
): TextField => ({ kind: 'text', key, label, help, maxLength });

const integer = (
	key: IntegerField['key'],
	label: string,
	help: string,
	min: number,
	max: number
): IntegerField => ({ kind: 'integer', key, label, help, min, max });

const blockList: BlockListField = {
	kind: 'uuid-list',
	key: 'block_ids',
	label: 'Document block IDs',
	help: 'Enter one active document-block UUID per line (up to 200).'
};

const stage: StageField = {
	kind: 'stage',
	key: 'stage',
	label: 'Screening stage',
	help: 'Choose the review queue where the proposal should be created.'
};

const reportId = uuid('report_id', 'Report ID', 'A UUID for a report in this project.');
const documentId = uuid('document_id', 'Document ID', 'A UUID for an attached document.');
const studyId = uuid('study_id', 'Study ID', 'A UUID for a study in this project.');
const definitionId = text(
	'definition_id',
	'Appraisal definition ID',
	'The nonempty appraisal-definition identifier.',
	100
);
const definitionVersion = integer(
	'definition_version',
	'Appraisal definition version',
	'A positive integer version.',
	1,
	2_147_483_647
);
const searchQuery = text('query', 'Search query', 'A nonempty search phrase.', 4_096);
const searchLimit = integer('limit', 'Result limit', 'How many results to return.', 1, 100);

const stringValue = (values: Readonly<ParsedToolValues>, key: FieldKey): string | undefined => {
	const value = values[key];
	return typeof value === 'string' ? value : undefined;
};

const numberValue = (values: Readonly<ParsedToolValues>, key: FieldKey): number | undefined => {
	const value = values[key];
	return typeof value === 'number' ? value : undefined;
};

const listValue = (values: Readonly<ParsedToolValues>, key: FieldKey): string[] | undefined => {
	const value = values[key];
	return Array.isArray(value) && value.every((item) => typeof item === 'string')
		? value
		: undefined;
};

const projectOnly: RequestBuilder = (project_id) => ({
	tool: 'get_project_protocol',
	args: { project_id }
});
const withReport = (project_id: string, values: Readonly<ParsedToolValues>) => {
	const report_id = stringValue(values, 'report_id');
	return report_id ? { project_id, report_id } : null;
};
const withStudy = (project_id: string, values: Readonly<ParsedToolValues>) => {
	const study_id = stringValue(values, 'study_id');
	return study_id ? { project_id, study_id } : null;
};
const withDocumentBlocks = (project_id: string, values: Readonly<ParsedToolValues>) => {
	const document_id = stringValue(values, 'document_id');
	const block_ids = listValue(values, 'block_ids');
	return document_id && block_ids ? { project_id, document_id, block_ids } : null;
};
const withDocumentSearch = (project_id: string, values: Readonly<ParsedToolValues>) => {
	const document_id = stringValue(values, 'document_id');
	const query = stringValue(values, 'query');
	const limit = numberValue(values, 'limit');
	return document_id && query && limit ? { project_id, document_id, query, limit } : null;
};
const withProjectSearch = (project_id: string, values: Readonly<ParsedToolValues>) => {
	const query = stringValue(values, 'query');
	const limit = numberValue(values, 'limit');
	return query && limit ? { project_id, query, limit } : null;
};
const withAppraisal = (project_id: string, values: Readonly<ParsedToolValues>) => {
	const report_id = stringValue(values, 'report_id');
	const definition_id = stringValue(values, 'definition_id');
	const definition_version = numberValue(values, 'definition_version');
	return report_id && definition_id && definition_version
		? { project_id, report_id, definition_id, definition_version }
		: null;
};
const withScreening = (
	project_id: string,
	values: Readonly<ParsedToolValues>
): ScreeningDecisionArgs | null => {
	const report_id = stringValue(values, 'report_id');
	const stage = stringValue(values, 'stage');
	return report_id && (stage === 'title_abstract' || stage === 'full_text')
		? { project_id, report_id, stage }
		: null;
};
const withDuplicate = (project_id: string, values: Readonly<ParsedToolValues>) => {
	const source_record_id = stringValue(values, 'source_record_id');
	const candidate_report_id = stringValue(values, 'candidate_report_id');
	return source_record_id && candidate_report_id
		? { project_id, source_record_id, candidate_report_id }
		: null;
};

export const ASSISTANT_TOOL_METADATA = {
	get_project_protocol: {
		name: 'get_project_protocol',
		kind: AssistantToolKind.read,
		label: 'Get project protocol',
		description: 'Read the published protocol for this project.',
		fields: [],
		reviewDestination: null,
		defaults: {},
		buildRequest: projectOnly
	},
	get_report: {
		name: 'get_report',
		kind: AssistantToolKind.read,
		label: 'Get report',
		description: 'Read one report scoped to this project.',
		fields: [reportId],
		reviewDestination: null,
		defaults: { report_id: '' },
		buildRequest: (projectId, values) => {
			const args = withReport(projectId, values);
			return args ? { tool: 'get_report', args } : null;
		}
	},
	read_document_blocks: {
		name: 'read_document_blocks',
		kind: AssistantToolKind.read,
		label: 'Read document blocks',
		description: 'Read selected active blocks from one document.',
		fields: [documentId, blockList],
		reviewDestination: null,
		defaults: { document_id: '', block_ids: '' },
		buildRequest: (projectId, values) => {
			const args = withDocumentBlocks(projectId, values);
			return args ? { tool: 'read_document_blocks', args } : null;
		}
	},
	search_document: {
		name: 'search_document',
		kind: AssistantToolKind.read,
		label: 'Search document',
		description: 'Search active blocks in one document.',
		fields: [documentId, searchQuery, searchLimit],
		reviewDestination: null,
		defaults: { document_id: '', query: '', limit: '20' },
		buildRequest: (projectId, values) => {
			const args = withDocumentSearch(projectId, values);
			return args ? { tool: 'search_document', args } : null;
		}
	},
	search_project_reports: {
		name: 'search_project_reports',
		kind: AssistantToolKind.read,
		label: 'Search project reports',
		description: 'Search report metadata within this project.',
		fields: [searchQuery, searchLimit],
		reviewDestination: null,
		defaults: { query: '', limit: '20' },
		buildRequest: (projectId, values) => {
			const args = withProjectSearch(projectId, values);
			return args ? { tool: 'search_project_reports', args } : null;
		}
	},
	get_screening_state: {
		name: 'get_screening_state',
		kind: AssistantToolKind.read,
		label: 'Get screening state',
		description: 'Read screening state for one report.',
		fields: [reportId],
		reviewDestination: null,
		defaults: { report_id: '' },
		buildRequest: (projectId, values) => {
			const args = withReport(projectId, values);
			return args ? { tool: 'get_screening_state', args } : null;
		}
	},
	get_study: {
		name: 'get_study',
		kind: AssistantToolKind.read,
		label: 'Get study',
		description: 'Read one study and its report membership.',
		fields: [studyId],
		reviewDestination: null,
		defaults: { study_id: '' },
		buildRequest: (projectId, values) => {
			const args = withStudy(projectId, values);
			return args ? { tool: 'get_study', args } : null;
		}
	},
	get_appraisal: {
		name: 'get_appraisal',
		kind: AssistantToolKind.read,
		label: 'Get appraisal',
		description: 'Read the latest completed appraisal version.',
		fields: [reportId, definitionId, definitionVersion],
		reviewDestination: null,
		defaults: { report_id: '', definition_id: '', definition_version: '1' },
		buildRequest: (projectId, values) => {
			const args = withAppraisal(projectId, values);
			return args ? { tool: 'get_appraisal', args } : null;
		}
	},
	propose_screening_decision: {
		name: 'propose_screening_decision',
		kind: AssistantToolKind.proposal,
		label: 'Propose screening decision',
		description: 'Generate a reviewer proposal for a screening decision.',
		fields: [reportId, stage],
		reviewDestination: 'screening',
		defaults: { report_id: '', stage: 'title_abstract' },
		buildRequest: (projectId, values) => {
			const args = withScreening(projectId, values);
			return args ? { tool: 'propose_screening_decision', args } : null;
		}
	},
	propose_duplicate_merge: {
		name: 'propose_duplicate_merge',
		kind: AssistantToolKind.proposal,
		label: 'Propose duplicate merge',
		description: 'Generate a reviewer proposal for a duplicate pair.',
		fields: [
			uuid('source_record_id', 'Source record ID', 'A UUID for the source record.'),
			uuid('candidate_report_id', 'Candidate report ID', 'A UUID for the candidate report.')
		],
		reviewDestination: 'deduplication',
		defaults: { source_record_id: '', candidate_report_id: '' },
		buildRequest: (projectId, values) => {
			const args = withDuplicate(projectId, values);
			return args ? { tool: 'propose_duplicate_merge', args } : null;
		}
	},
	propose_study_grouping: {
		name: 'propose_study_grouping',
		kind: AssistantToolKind.proposal,
		label: 'Propose study grouping',
		description: 'Generate a reviewer proposal for report grouping.',
		fields: [reportId],
		reviewDestination: 'studies',
		defaults: { report_id: '' },
		buildRequest: (projectId, values) => {
			const args = withReport(projectId, values);
			return args ? { tool: 'propose_study_grouping', args } : null;
		}
	},
	propose_classification: {
		name: 'propose_classification',
		kind: AssistantToolKind.proposal,
		label: 'Propose classification',
		description: 'Generate a reviewer proposal for study design.',
		fields: [studyId],
		reviewDestination: 'studies',
		defaults: { study_id: '' },
		buildRequest: (projectId, values) => {
			const args = withStudy(projectId, values);
			return args ? { tool: 'propose_classification', args } : null;
		}
	},
	propose_extraction: {
		name: 'propose_extraction',
		kind: AssistantToolKind.proposal,
		label: 'Propose extraction',
		description: 'Generate a reviewer proposal for data extraction.',
		fields: [studyId],
		reviewDestination: 'extraction',
		defaults: { study_id: '' },
		buildRequest: (projectId, values) => {
			const args = withStudy(projectId, values);
			return args ? { tool: 'propose_extraction', args } : null;
		}
	},
	propose_appraisal_answer: {
		name: 'propose_appraisal_answer',
		kind: AssistantToolKind.proposal,
		label: 'Propose appraisal answer',
		description: 'Generate a reviewer proposal for appraisal answers.',
		fields: [reportId, definitionId, definitionVersion],
		reviewDestination: 'appraisal',
		defaults: { report_id: '', definition_id: '', definition_version: '1' },
		buildRequest: (projectId, values) => {
			const args = withAppraisal(projectId, values);
			return args ? { tool: 'propose_appraisal_answer', args } : null;
		}
	}
} satisfies Record<ToolName, ToolDefinition>;

export function isAssistantToolName(value: unknown): value is ToolName {
	return typeof value === 'string' && ASSISTANT_TOOL_NAMES.some((name) => name === value);
}

export function partitionAssistantCatalog(catalog: readonly AssistantToolDescriptor[]): {
	supported: SupportedCatalogEntry[];
	unsupported: AssistantToolDescriptor[];
} {
	const supported: SupportedCatalogEntry[] = [];
	const unsupported: AssistantToolDescriptor[] = [];

	for (const descriptor of catalog) {
		if (isAssistantToolName(descriptor.name)) {
			const metadata = ASSISTANT_TOOL_METADATA[descriptor.name];
			if (descriptor.kind === metadata.kind) {
				supported.push({ metadata, descriptor });
				continue;
			}
		}
		unsupported.push(descriptor);
	}

	return { supported, unsupported };
}

export function initialToolValues(tool: ToolName): ToolValues {
	return { ...ASSISTANT_TOOL_METADATA[tool].defaults };
}

export function serializeToolRequest(
	tool: ToolName,
	projectId: string,
	values: ToolValues
): ToolValidation {
	const errors: Record<string, string> = {};
	if (!projectId.trim()) errors.project_id = 'Project scope is missing.';
	const parsed = parseToolValues(tool, values, errors);
	if (Object.keys(errors).length > 0) return { kind: 'invalid', errors };
	const args = ASSISTANT_TOOL_METADATA[tool].buildRequest(projectId, parsed);
	if (!args) return { kind: 'invalid', errors: { form: 'Tool form configuration is invalid.' } };
	return { kind: 'valid', request: args };
}

function parseToolValues(
	tool: ToolName,
	values: ToolValues,
	errors: Record<string, string>
): ParsedToolValues {
	const parsed: ParsedToolValues = {};
	for (const field of ASSISTANT_TOOL_METADATA[tool].fields) {
		const result = parseFieldValue(field, values[field.key] ?? '');
		if (result.kind === 'valid') parsed[field.key] = result.value;
		else errors[field.key] = result.message;
	}
	return parsed;
}

function parseFieldValue(field: ToolField, raw: string): FieldValidation {
	switch (field.kind) {
		case 'uuid':
			return parseUuidField(raw);
		case 'text':
			return parseTextField(field, raw);
		case 'integer':
			return parseIntegerField(field, raw);
		case 'uuid-list':
			return parseUuidListField(raw);
		case 'stage':
			return parseStageField(raw);
		default: {
			const exhaustive: never = field;
			return exhaustive;
		}
	}
}

function parseUuidField(raw: string): FieldValidation {
	const value = raw.trim();
	return isUuid(value)
		? { kind: 'valid', value }
		: { kind: 'invalid', message: 'Enter a valid UUID.' };
}

function parseTextField(field: TextField, raw: string): FieldValidation {
	const value = raw.trim();
	if (!value) return { kind: 'invalid', message: 'This value is required.' };
	if (value.length > field.maxLength)
		return { kind: 'invalid', message: `Use at most ${field.maxLength} characters.` };
	return { kind: 'valid', value };
}

function parseIntegerField(field: IntegerField, raw: string): FieldValidation {
	const value = Number(raw);
	return Number.isInteger(value) && value >= field.min && value <= field.max
		? { kind: 'valid', value }
		: {
				kind: 'invalid',
				message: `Enter a whole number from ${field.min} to ${field.max}.`
			};
}

function parseUuidListField(raw: string): FieldValidation {
	const value = raw
		.split(/\r?\n/)
		.map((item) => item.trim())
		.filter(Boolean);
	if (value.length === 0)
		return { kind: 'invalid', message: 'Enter at least one document-block UUID.' };
	if (value.length > 200)
		return { kind: 'invalid', message: 'Enter no more than 200 document-block UUIDs.' };
	if (value.some((item) => !isUuid(item)))
		return { kind: 'invalid', message: 'Every document-block entry must be a valid UUID.' };
	return { kind: 'valid', value };
}

function parseStageField(raw: string): FieldValidation {
	return raw === 'title_abstract' || raw === 'full_text'
		? { kind: 'valid', value: raw }
		: { kind: 'invalid', message: 'Choose a screening stage.' };
}

export function reviewPath(tool: ToolName, projectId: string, values: ToolValues): string | null {
	const destination = ASSISTANT_TOOL_METADATA[tool].reviewDestination;
	if (!destination) return null;
	if (destination === 'screening') {
		return `/projects/${encodeURIComponent(projectId)}/screening/${
			values.stage === 'full_text' ? 'full-text' : 'title-abstract'
		}`;
	}
	if (destination === 'studies') {
		const query =
			tool === 'propose_classification'
				? `?study=${encodeURIComponent(values.study_id ?? '')}`
				: `?report=${encodeURIComponent(values.report_id ?? '')}`;
		return `/projects/${encodeURIComponent(projectId)}/studies${query}`;
	}
	return `/projects/${encodeURIComponent(projectId)}/${destination === 'deduplication' ? 'discovery/duplicates' : destination}`;
}

function isUuid(value: string): boolean {
	return /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value);
}
