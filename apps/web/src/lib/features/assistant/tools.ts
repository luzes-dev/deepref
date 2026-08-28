import { AssistantToolKind, AssistantToolNameDto } from '$lib/api/generated/models';
import type {
	AssistantToolDescriptor,
	AssistantToolRequest,
	AssistantToolRequestArgs,
	AssistantToolNameDto as GeneratedToolName
} from '$lib/api/generated/models';

export type ToolName = GeneratedToolName;
export type ToolKind = (typeof AssistantToolKind)[keyof typeof AssistantToolKind];
export type ScreeningStage = 'title_abstract' | 'full_text';
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

export type ToolValues = Record<string, string>;

export type ToolValidation =
	| { kind: 'valid'; request: AssistantToolRequest }
	| { kind: 'invalid'; errors: Readonly<Record<string, string>> };

export type SupportedCatalogEntry = {
	metadata: ToolMetadata;
	descriptor: AssistantToolDescriptor;
};

export const ASSISTANT_TOOL_NAMES = [
	AssistantToolNameDto.get_project_protocol,
	AssistantToolNameDto.get_report,
	AssistantToolNameDto.read_document_blocks,
	AssistantToolNameDto.search_document,
	AssistantToolNameDto.search_project_reports,
	AssistantToolNameDto.get_screening_state,
	AssistantToolNameDto.get_study,
	AssistantToolNameDto.get_appraisal,
	AssistantToolNameDto.propose_screening_decision,
	AssistantToolNameDto.propose_duplicate_merge,
	AssistantToolNameDto.propose_study_grouping,
	AssistantToolNameDto.propose_classification,
	AssistantToolNameDto.propose_extraction,
	AssistantToolNameDto.propose_appraisal_answer
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

export const ASSISTANT_TOOL_METADATA = {
	get_project_protocol: {
		name: AssistantToolNameDto.get_project_protocol,
		kind: AssistantToolKind.read,
		label: 'Get project protocol',
		description: 'Read the published protocol for this project.',
		fields: [],
		reviewDestination: null
	},
	get_report: {
		name: AssistantToolNameDto.get_report,
		kind: AssistantToolKind.read,
		label: 'Get report',
		description: 'Read one report scoped to this project.',
		fields: [reportId],
		reviewDestination: null
	},
	read_document_blocks: {
		name: AssistantToolNameDto.read_document_blocks,
		kind: AssistantToolKind.read,
		label: 'Read document blocks',
		description: 'Read selected active blocks from one document.',
		fields: [documentId, blockList],
		reviewDestination: null
	},
	search_document: {
		name: AssistantToolNameDto.search_document,
		kind: AssistantToolKind.read,
		label: 'Search document',
		description: 'Search active blocks in one document.',
		fields: [documentId, searchQuery, searchLimit],
		reviewDestination: null
	},
	search_project_reports: {
		name: AssistantToolNameDto.search_project_reports,
		kind: AssistantToolKind.read,
		label: 'Search project reports',
		description: 'Search report metadata within this project.',
		fields: [searchQuery, searchLimit],
		reviewDestination: null
	},
	get_screening_state: {
		name: AssistantToolNameDto.get_screening_state,
		kind: AssistantToolKind.read,
		label: 'Get screening state',
		description: 'Read screening state for one report.',
		fields: [reportId],
		reviewDestination: null
	},
	get_study: {
		name: AssistantToolNameDto.get_study,
		kind: AssistantToolKind.read,
		label: 'Get study',
		description: 'Read one study and its report membership.',
		fields: [studyId],
		reviewDestination: null
	},
	get_appraisal: {
		name: AssistantToolNameDto.get_appraisal,
		kind: AssistantToolKind.read,
		label: 'Get appraisal',
		description: 'Read the latest completed appraisal version.',
		fields: [reportId, definitionId, definitionVersion],
		reviewDestination: null
	},
	propose_screening_decision: {
		name: AssistantToolNameDto.propose_screening_decision,
		kind: AssistantToolKind.proposal,
		label: 'Propose screening decision',
		description: 'Generate a reviewer proposal for a screening decision.',
		fields: [reportId, stage],
		reviewDestination: 'screening'
	},
	propose_duplicate_merge: {
		name: AssistantToolNameDto.propose_duplicate_merge,
		kind: AssistantToolKind.proposal,
		label: 'Propose duplicate merge',
		description: 'Generate a reviewer proposal for a duplicate pair.',
		fields: [
			uuid('source_record_id', 'Source record ID', 'A UUID for the source record.'),
			uuid('candidate_report_id', 'Candidate report ID', 'A UUID for the candidate report.')
		],
		reviewDestination: 'deduplication'
	},
	propose_study_grouping: {
		name: AssistantToolNameDto.propose_study_grouping,
		kind: AssistantToolKind.proposal,
		label: 'Propose study grouping',
		description: 'Generate a reviewer proposal for report grouping.',
		fields: [reportId],
		reviewDestination: 'studies'
	},
	propose_classification: {
		name: AssistantToolNameDto.propose_classification,
		kind: AssistantToolKind.proposal,
		label: 'Propose classification',
		description: 'Generate a reviewer proposal for study design.',
		fields: [studyId],
		reviewDestination: 'studies'
	},
	propose_extraction: {
		name: AssistantToolNameDto.propose_extraction,
		kind: AssistantToolKind.proposal,
		label: 'Propose extraction',
		description: 'Generate a reviewer proposal for data extraction.',
		fields: [studyId],
		reviewDestination: 'extraction'
	},
	propose_appraisal_answer: {
		name: AssistantToolNameDto.propose_appraisal_answer,
		kind: AssistantToolKind.proposal,
		label: 'Propose appraisal answer',
		description: 'Generate a reviewer proposal for appraisal answers.',
		fields: [reportId, definitionId, definitionVersion],
		reviewDestination: 'appraisal'
	}
} satisfies Record<ToolName, ToolMetadata>;

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
	switch (tool) {
		case AssistantToolNameDto.get_project_protocol:
			return {};
		case AssistantToolNameDto.get_report:
		case AssistantToolNameDto.get_screening_state:
		case AssistantToolNameDto.propose_study_grouping:
			return { report_id: '' };
		case AssistantToolNameDto.read_document_blocks:
			return { document_id: '', block_ids: '' };
		case AssistantToolNameDto.search_document:
			return { document_id: '', query: '', limit: '20' };
		case AssistantToolNameDto.search_project_reports:
			return { query: '', limit: '20' };
		case AssistantToolNameDto.get_study:
		case AssistantToolNameDto.propose_classification:
		case AssistantToolNameDto.propose_extraction:
			return { study_id: '' };
		case AssistantToolNameDto.get_appraisal:
		case AssistantToolNameDto.propose_appraisal_answer:
			return { report_id: '', definition_id: '', definition_version: '1' };
		case AssistantToolNameDto.propose_screening_decision:
			return { report_id: '', stage: 'title_abstract' };
		case AssistantToolNameDto.propose_duplicate_merge:
			return { source_record_id: '', candidate_report_id: '' };
		default: {
			const exhaustive: never = tool;
			return exhaustive;
		}
	}
}

export function serializeToolRequest(
	tool: ToolName,
	projectId: string,
	values: ToolValues
): ToolValidation {
	const errors: Record<string, string> = {};
	if (!projectId.trim()) errors.project_id = 'Project scope is missing.';

	const requiredUuid = (key: UuidField['key']): string | undefined => {
		const value = values[key]?.trim() ?? '';
		if (!isUuid(value)) {
			errors[key] = 'Enter a valid UUID.';
			return undefined;
		}
		return value;
	};
	const requiredText = (key: TextField['key'], maxLength: number): string | undefined => {
		const value = values[key]?.trim() ?? '';
		if (!value) errors[key] = 'This value is required.';
		else if (value.length > maxLength) errors[key] = `Use at most ${maxLength} characters.`;
		return value && value.length <= maxLength ? value : undefined;
	};
	const requiredInteger = (
		key: IntegerField['key'],
		min: number,
		max: number
	): number | undefined => {
		const value = Number(values[key] ?? '');
		if (!Number.isInteger(value) || value < min || value > max) {
			errors[key] = `Enter a whole number from ${min} to ${max}.`;
			return undefined;
		}
		return value;
	};
	const requiredStage = (): ScreeningStage | undefined => {
		const value = values.stage;
		if (value !== 'title_abstract' && value !== 'full_text') {
			errors.stage = 'Choose a screening stage.';
			return undefined;
		}
		return value;
	};
	const requiredBlockIds = (): string[] | undefined => {
		const blockIds = (values.block_ids ?? '')
			.split(/\r?\n/)
			.map((value) => value.trim())
			.filter((value) => value.length > 0);
		if (blockIds.length === 0) {
			errors.block_ids = 'Enter at least one document-block UUID.';
			return undefined;
		}
		if (blockIds.length > 200) {
			errors.block_ids = 'Enter no more than 200 document-block UUIDs.';
			return undefined;
		}
		if (blockIds.some((value) => !isUuid(value))) {
			errors.block_ids = 'Every document-block entry must be a valid UUID.';
			return undefined;
		}
		return blockIds;
	};

	if (Object.keys(errors).length > 0) return { kind: 'invalid', errors };

	switch (tool) {
		case AssistantToolNameDto.get_project_protocol:
			return valid(tool, projectId, { project_id: projectId });
		case AssistantToolNameDto.get_report: {
			const reportId = requiredUuid('report_id');
			return reportId
				? valid(tool, projectId, { project_id: projectId, report_id: reportId })
				: { kind: 'invalid', errors };
		}
		case AssistantToolNameDto.read_document_blocks: {
			const documentId = requiredUuid('document_id');
			const blockIds = requiredBlockIds();
			return documentId && blockIds
				? valid(tool, projectId, {
						project_id: projectId,
						document_id: documentId,
						block_ids: blockIds
					})
				: { kind: 'invalid', errors };
		}
		case AssistantToolNameDto.search_document: {
			const documentId = requiredUuid('document_id');
			const query = requiredText('query', 4_096);
			const limit = requiredInteger('limit', 1, 100);
			return documentId && query && limit
				? valid(tool, projectId, {
						project_id: projectId,
						document_id: documentId,
						query,
						limit
					})
				: { kind: 'invalid', errors };
		}
		case AssistantToolNameDto.search_project_reports: {
			const query = requiredText('query', 4_096);
			const limit = requiredInteger('limit', 1, 100);
			return query && limit
				? valid(tool, projectId, { project_id: projectId, query, limit })
				: { kind: 'invalid', errors };
		}
		case AssistantToolNameDto.get_screening_state: {
			const reportId = requiredUuid('report_id');
			return reportId
				? valid(tool, projectId, { project_id: projectId, report_id: reportId })
				: { kind: 'invalid', errors };
		}
		case AssistantToolNameDto.get_study:
		case AssistantToolNameDto.propose_classification:
		case AssistantToolNameDto.propose_extraction: {
			const studyId = requiredUuid('study_id');
			return studyId
				? valid(tool, projectId, { project_id: projectId, study_id: studyId })
				: { kind: 'invalid', errors };
		}
		case AssistantToolNameDto.get_appraisal:
		case AssistantToolNameDto.propose_appraisal_answer: {
			const reportId = requiredUuid('report_id');
			const definitionId = requiredText('definition_id', 100);
			const definitionVersion = requiredInteger('definition_version', 1, 2_147_483_647);
			return reportId && definitionId && definitionVersion
				? valid(tool, projectId, {
						project_id: projectId,
						report_id: reportId,
						definition_id: definitionId,
						definition_version: definitionVersion
					})
				: { kind: 'invalid', errors };
		}
		case AssistantToolNameDto.propose_screening_decision: {
			const reportId = requiredUuid('report_id');
			const stage = requiredStage();
			return reportId && stage
				? valid(tool, projectId, { project_id: projectId, report_id: reportId, stage })
				: { kind: 'invalid', errors };
		}
		case AssistantToolNameDto.propose_duplicate_merge: {
			const sourceRecordId = requiredUuid('source_record_id');
			const candidateReportId = requiredUuid('candidate_report_id');
			return sourceRecordId && candidateReportId
				? valid(tool, projectId, {
						project_id: projectId,
						source_record_id: sourceRecordId,
						candidate_report_id: candidateReportId
					})
				: { kind: 'invalid', errors };
		}
		case AssistantToolNameDto.propose_study_grouping: {
			const reportId = requiredUuid('report_id');
			return reportId
				? valid(tool, projectId, { project_id: projectId, report_id: reportId })
				: { kind: 'invalid', errors };
		}
		default: {
			const exhaustive: never = tool;
			return exhaustive;
		}
	}
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
			tool === AssistantToolNameDto.propose_classification
				? `?study=${encodeURIComponent(values.study_id ?? '')}`
				: `?report=${encodeURIComponent(values.report_id ?? '')}`;
		return `/projects/${encodeURIComponent(projectId)}/studies${query}`;
	}
	return `/projects/${encodeURIComponent(projectId)}/${destination === 'deduplication' ? 'discovery/duplicates' : destination}`;
}

function valid(tool: ToolName, projectId: string, args: AssistantToolRequestArgs): ToolValidation {
	return { kind: 'valid', request: { tool, args: { ...args, project_id: projectId } } };
}

function isUuid(value: string): boolean {
	return /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value);
}
