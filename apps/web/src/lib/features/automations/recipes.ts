import type { ToolField, ToolName } from '$lib/features/assistant/tools';

export type RecipeCategory =
	'Screening' | 'Studies & Synthesis' | 'Document Analysis' | 'Maintenance';

export const RECIPE_CATEGORIES: readonly RecipeCategory[] = [
	'Screening',
	'Studies & Synthesis',
	'Document Analysis',
	'Maintenance'
] as const;

export const BACKEND_RECIPES = {
	project_maintenance: 'project_maintenance',
	review_screening: 'review_screening',
	review_duplicate_detection: 'review_duplicate_detection',
	review_study_classification: 'review_study_classification',
	review_study_grouping: 'review_study_grouping',
	review_appraisal_prefill: 'review_appraisal_prefill',
	review_data_extraction: 'review_data_extraction'
} as const;

export type BackendRecipeId = (typeof BACKEND_RECIPES)[keyof typeof BACKEND_RECIPES];

export const BACKEND_RECIPE_ROUTES: Record<BackendRecipeId, string> = {
	project_maintenance: 'project_maintenance.v1',
	review_screening: 'review_screening.v1',
	review_duplicate_detection: 'review_duplicate_detection.v1',
	review_study_classification: 'review_study_classification.v1',
	review_study_grouping: 'review_study_grouping.v1',
	review_appraisal_prefill: 'review_appraisal_prefill.v1',
	review_data_extraction: 'review_data_extraction.v1'
} as const;

export type ReviewDestination =
	'screening' | 'deduplication' | 'studies' | 'extraction' | 'appraisal';

export interface PredefinedRecipe {
	readonly id: string;
	readonly toolName: ToolName;
	readonly title: string;
	readonly description: string;
	readonly category: RecipeCategory;
	readonly icon: string;
	readonly backendRecipe: BackendRecipeId;
	readonly backendRoute: string;
	readonly fields: readonly ToolField[];
	readonly defaultParameters: Readonly<Record<string, string>>;
	readonly reviewDestination: ReviewDestination | null;
}

// Field descriptors matching Assistant tools
const uuid = (
	key: 'report_id' | 'document_id' | 'source_record_id' | 'candidate_report_id' | 'study_id',
	label: string,
	help: string
): ToolField => ({
	kind: 'uuid',
	key,
	label,
	help
});

const text = (
	key: 'query' | 'definition_id',
	label: string,
	help: string,
	maxLength: number
): ToolField => ({
	kind: 'text',
	key,
	label,
	help,
	maxLength
});

const integer = (
	key: 'limit' | 'definition_version',
	label: string,
	help: string,
	min: number,
	max: number
): ToolField => ({
	kind: 'integer',
	key,
	label,
	help,
	min,
	max
});

const blockList: ToolField = {
	kind: 'uuid-list',
	key: 'block_ids',
	label: 'Document block IDs',
	help: 'Enter one active document-block UUID per line (up to 200).'
};

const stage: ToolField = {
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
const searchLimit = integer('limit', 'Result limit', 'How many results to return (1-100).', 1, 100);

export const PREDEFINED_RECIPES: readonly PredefinedRecipe[] = [
	// --- Screening Recipes ---
	{
		id: 'single_report_screening',
		toolName: 'propose_screening_decision',
		title: 'Single Report Screening',
		description:
			'Generate an AI screening proposal with citation-grounded inclusion or exclusion rationale.',
		category: 'Screening',
		icon: 'CheckCircle2',
		backendRecipe: 'review_screening',
		backendRoute: BACKEND_RECIPE_ROUTES.review_screening,
		fields: [reportId, stage],
		defaultParameters: { report_id: '', stage: 'title_abstract' },
		reviewDestination: 'screening'
	},
	{
		id: 'duplicate_detection',
		toolName: 'propose_duplicate_merge',
		title: 'Duplicate Detection',
		description:
			'Detect duplicate bibliographic records and generate a merge proposal for reviewer sign-off.',
		category: 'Screening',
		icon: 'Copy',
		backendRecipe: 'review_duplicate_detection',
		backendRoute: BACKEND_RECIPE_ROUTES.review_duplicate_detection,
		fields: [
			uuid('source_record_id', 'Source record ID', 'A UUID for the source record.'),
			uuid('candidate_report_id', 'Candidate report ID', 'A UUID for the candidate report.')
		],
		defaultParameters: { source_record_id: '', candidate_report_id: '' },
		reviewDestination: 'deduplication'
	},
	{
		id: 'get_screening_state',
		toolName: 'get_screening_state',
		title: 'Screening State Inspection',
		description:
			'Read screening decisions, exclusion codes, and reviewer conflicts for a specific report.',
		category: 'Screening',
		icon: 'FileCheck',
		backendRecipe: 'review_screening',
		backendRoute: BACKEND_RECIPE_ROUTES.review_screening,
		fields: [reportId],
		defaultParameters: { report_id: '' },
		reviewDestination: null
	},

	// --- Studies & Synthesis Recipes ---
	{
		id: 'study_grouping',
		toolName: 'propose_study_grouping',
		title: 'Study Grouping',
		description: 'Synthesize related multi-report publications into unified clinical studies.',
		category: 'Studies & Synthesis',
		icon: 'FolderTree',
		backendRecipe: 'review_study_grouping',
		backendRoute: BACKEND_RECIPE_ROUTES.review_study_grouping,
		fields: [reportId],
		defaultParameters: { report_id: '' },
		reviewDestination: 'studies'
	},
	{
		id: 'study_classification',
		toolName: 'propose_classification',
		title: 'Study Classification',
		description: 'Classify study design, population, interventions, and comparator arms.',
		category: 'Studies & Synthesis',
		icon: 'Tags',
		backendRecipe: 'review_study_classification',
		backendRoute: BACKEND_RECIPE_ROUTES.review_study_classification,
		fields: [studyId],
		defaultParameters: { study_id: '' },
		reviewDestination: 'studies'
	},
	{
		id: 'appraisal_prefill',
		toolName: 'propose_appraisal_answer',
		title: 'Appraisal Prefill',
		description:
			'Prefill critical appraisal answers (e.g. RoB 2, ROBINS-I) with grounded evidence citations.',
		category: 'Studies & Synthesis',
		icon: 'ShieldAlert',
		backendRecipe: 'review_appraisal_prefill',
		backendRoute: BACKEND_RECIPE_ROUTES.review_appraisal_prefill,
		fields: [reportId, definitionId, definitionVersion],
		defaultParameters: { report_id: '', definition_id: '', definition_version: '1' },
		reviewDestination: 'appraisal'
	},
	{
		id: 'get_study',
		toolName: 'get_study',
		title: 'Study Details Inspection',
		description: 'Read one study and its report membership scoped to this project.',
		category: 'Studies & Synthesis',
		icon: 'BookOpen',
		backendRecipe: 'review_study_grouping',
		backendRoute: BACKEND_RECIPE_ROUTES.review_study_grouping,
		fields: [studyId],
		defaultParameters: { study_id: '' },
		reviewDestination: null
	},
	{
		id: 'get_appraisal',
		toolName: 'get_appraisal',
		title: 'Appraisal Status Check',
		description: 'Read the latest completed appraisal version and reviewer responses.',
		category: 'Studies & Synthesis',
		icon: 'ClipboardCheck',
		backendRecipe: 'review_appraisal_prefill',
		backendRoute: BACKEND_RECIPE_ROUTES.review_appraisal_prefill,
		fields: [reportId, definitionId, definitionVersion],
		defaultParameters: { report_id: '', definition_id: '', definition_version: '1' },
		reviewDestination: null
	},

	// --- Document Analysis Recipes ---
	{
		id: 'document_extraction',
		toolName: 'propose_extraction',
		title: 'Document Extraction',
		description:
			'Extract targeted outcome data, baseline characteristics, and numerical metrics.',
		category: 'Document Analysis',
		icon: 'FileSpreadsheet',
		backendRecipe: 'review_data_extraction',
		backendRoute: BACKEND_RECIPE_ROUTES.review_data_extraction,
		fields: [studyId],
		defaultParameters: { study_id: '' },
		reviewDestination: 'extraction'
	},
	{
		id: 'document_search',
		toolName: 'search_document',
		title: 'Document Search',
		description: 'Search active blocks and extracted content in a specific attached document.',
		category: 'Document Analysis',
		icon: 'FileSearch',
		backendRecipe: 'project_maintenance',
		backendRoute: BACKEND_RECIPE_ROUTES.project_maintenance,
		fields: [documentId, searchQuery, searchLimit],
		defaultParameters: { document_id: '', query: '', limit: '20' },
		reviewDestination: null
	},
	{
		id: 'report_search',
		toolName: 'search_project_reports',
		title: 'Report Search',
		description: 'Search report metadata, titles, and abstracts across the entire project.',
		category: 'Document Analysis',
		icon: 'Search',
		backendRecipe: 'project_maintenance',
		backendRoute: BACKEND_RECIPE_ROUTES.project_maintenance,
		fields: [searchQuery, searchLimit],
		defaultParameters: { query: '', limit: '20' },
		reviewDestination: null
	},
	{
		id: 'read_document_blocks',
		toolName: 'read_document_blocks',
		title: 'Read Document Blocks',
		description: 'Read selected active blocks from one document to verify text extraction.',
		category: 'Document Analysis',
		icon: 'Layers',
		backendRecipe: 'project_maintenance',
		backendRoute: BACKEND_RECIPE_ROUTES.project_maintenance,
		fields: [documentId, blockList],
		defaultParameters: { document_id: '', block_ids: '' },
		reviewDestination: null
	},
	{
		id: 'get_report',
		toolName: 'get_report',
		title: 'Report Inspector',
		description:
			'Read one report scoped to this project, including metadata and document links.',
		category: 'Document Analysis',
		icon: 'FileText',
		backendRecipe: 'project_maintenance',
		backendRoute: BACKEND_RECIPE_ROUTES.project_maintenance,
		fields: [reportId],
		defaultParameters: { report_id: '' },
		reviewDestination: null
	},

	// --- Maintenance Recipes ---
	{
		id: 'get_project_protocol',
		toolName: 'get_project_protocol',
		title: 'Project Protocol Audit',
		description:
			'Read and audit the published protocol and criteria framework for this project.',
		category: 'Maintenance',
		icon: 'ShieldCheck',
		backendRecipe: 'project_maintenance',
		backendRoute: BACKEND_RECIPE_ROUTES.project_maintenance,
		fields: [],
		defaultParameters: {},
		reviewDestination: null
	}
] as const;

export const UUID_PATTERN =
	/^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

export function isUuid(value: string): boolean {
	return UUID_PATTERN.test(value.trim());
}

export type ParsedRecipeValues = Record<string, string | number | string[]>;

export type RecipeValidationResult =
	{ valid: true; parsed: ParsedRecipeValues } | { valid: false; errors: Record<string, string> };

export function validateRecipeParameters(
	recipe: PredefinedRecipe,
	projectId: string,
	values: Record<string, string>
): RecipeValidationResult {
	const errors: Record<string, string> = {};

	if (!projectId.trim()) {
		errors.project_id = 'Project scope is required.';
	}

	const parsed: ParsedRecipeValues = {};

	for (const field of recipe.fields) {
		const raw = values[field.key]?.trim() ?? '';

		switch (field.kind) {
			case 'uuid': {
				if (!raw) {
					errors[field.key] = `${field.label} is required.`;
				} else if (!isUuid(raw)) {
					errors[field.key] = `${field.label} must be a valid UUID.`;
				} else {
					parsed[field.key] = raw;
				}
				break;
			}
			case 'stage': {
				if (raw !== 'title_abstract' && raw !== 'full_text') {
					errors[field.key] = 'Choose title_abstract or full_text.';
				} else {
					parsed[field.key] = raw;
				}
				break;
			}
			case 'text': {
				if (!raw) {
					errors[field.key] = `${field.label} cannot be blank.`;
				} else if (raw.length > field.maxLength) {
					errors[field.key] =
						`${field.label} must be ${field.maxLength} characters or fewer.`;
				} else {
					parsed[field.key] = raw;
				}
				break;
			}
			case 'integer': {
				if (!raw) {
					errors[field.key] = `${field.label} is required.`;
				} else {
					const parsedNumber = Number(raw);
					if (
						!Number.isInteger(parsedNumber) ||
						parsedNumber < field.min ||
						parsedNumber > field.max
					) {
						errors[field.key] =
							`${field.label} must be an integer between ${field.min} and ${field.max}.`;
					} else {
						parsed[field.key] = parsedNumber;
					}
				}
				break;
			}
			case 'uuid-list': {
				const lines = raw
					.split(/[\r\n,]+/)
					.map((item) => item.trim())
					.filter((item) => item.length > 0);
				if (lines.length === 0) {
					errors[field.key] = 'Provide at least one block UUID.';
				} else if (lines.length > 200) {
					errors[field.key] = 'Enter no more than 200 block UUIDs.';
				} else if (lines.some((item) => !isUuid(item))) {
					errors[field.key] = 'Every block ID must be a valid UUID.';
				} else {
					parsed[field.key] = lines;
				}
				break;
			}
		}
	}

	if (Object.keys(errors).length > 0) {
		return { valid: false, errors };
	}

	return { valid: true, parsed };
}

export function getRecipeById(id: string): PredefinedRecipe | undefined {
	return PREDEFINED_RECIPES.find((recipe) => recipe.id === id);
}

export function getRecipesByCategory(category: RecipeCategory): readonly PredefinedRecipe[] {
	return PREDEFINED_RECIPES.filter((recipe) => recipe.category === category);
}

export function initialRecipeParameters(recipe: PredefinedRecipe): Record<string, string> {
	return { ...recipe.defaultParameters };
}

export function recipeForTool(toolName: ToolName): PredefinedRecipe | undefined {
	return PREDEFINED_RECIPES.find((recipe) => recipe.toolName === toolName);
}
