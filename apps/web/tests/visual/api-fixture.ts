import type {
	AiActivityOverviewDto,
	AiBudgetDto,
	AiStatusDto,
	AppraisalDefinitionDto,
	AssistantConversationDto,
	DedupeProposalDto,
	DependencyStatus,
	DocumentDto,
	EligibilityCriterionDto,
	ExtractionFieldDto,
	FullTextExclusionReasonDto,
	FullTextQueueDto,
	IngestionDto,
	MissingFullTextDto,
	NodeTypeDto,
	NotificationUnreadDto,
	PaginatedResponseAcquisitionDto,
	PaginatedResponseAiActivityDto,
	PaginatedResponseAiProposalDto,
	PaginatedResponseDedupeProposalDto,
	PaginatedResponseIngestionDto,
	PaginatedResponseNotificationDto,
	PaginatedResponseProjectDto,
	PaginatedResponseReportDto,
	PrismaDto,
	ProjectDto,
	ProjectionStatusDto,
	ProtocolDto,
	ProtocolVersionsResponse,
	ReportDetailDto,
	ReportDto,
	ScreeningHistoryDto,
	ScreeningQueueDto,
	SettingsDto,
	StudyListDto,
	TemplateDto,
	UngroupedReportDto,
	WorkflowDto,
	WorkflowGraphDto,
	WorkflowRunDto
} from '../../src/lib/api/generated/models';

export const VISUAL_PROJECT_ID = 'visual-project';
const VISUAL_REPORT_ID = '00000000-0000-4000-8000-000000000001';
const VISUAL_NOW = '2026-01-15T12:00:00Z';
const PROJECT_TIMESTAMP = '2026-01-01T00:00:00Z';

/**
 * One GET the deterministic workspace answers. `query` is the exact canonical query string the
 * app sends (`URLSearchParams` order, empty when there is none). Anything that does not match a
 * listed path and query is refused by the fixture, so a new request has to be modelled here.
 */
export type VisualEndpoint = {
	readonly path: string;
	readonly query: string;
	readonly body: unknown;
};

const reportTitles = [
	'Effects of evidence mapping on review quality',
	'Citation networks for transparent synthesis',
	'Human-centred methods for systematic reviews',
	'Reproducible retrieval across scholarly databases'
] as const;

const project: ProjectDto = {
	id: VISUAL_PROJECT_ID,
	name: 'Evidence synthesis workspace',
	description: 'A stable fixture for shell and accessibility review.',
	default_max_depth: 2,
	article_count: 12,
	created_at: PROJECT_TIMESTAMP,
	updated_at: VISUAL_NOW
};

function fixtureReport(index: number): ReportDto {
	return {
		report_id:
			index === 0
				? VISUAL_REPORT_ID
				: `00000000-0000-4000-8000-${String(index + 1).padStart(12, '0')}`,
		doi: `10.5555/deepref-fixture-${String(index + 1).padStart(2, '0')}`,
		title: reportTitles[index % reportTitles.length],
		issued_year: 2020 + (index % 5),
		type: index % 3 === 0 ? 'review' : 'article',
		total_citations: 18 + index * 3,
		internal_citations: index % 6,
		outbound_internal_references: index % 4,
		rank_score: Number((0.96 - index * 0.045).toFixed(3)),
		metrics_as_of: VISUAL_NOW,
		metrics_stale: false
	};
}

const reports: ReportDto[] = Array.from({ length: 12 }, (_, index) => fixtureReport(index));
const leadReport = fixtureReport(0);

const ingestions: IngestionDto[] = [
	{
		id: 'visual-ingestion',
		project_id: VISUAL_PROJECT_ID,
		status: 'completed',
		seed_count: 2,
		fetched_count: 24,
		failed_count: 1,
		queued_count: 0,
		max_depth: 2,
		created_at: '2026-01-14T09:30:00Z',
		started_at: '2026-01-14T09:30:02Z',
		completed_at: '2026-01-14T09:31:45Z'
	},
	{
		id: 'visual-ingestion-previous',
		project_id: VISUAL_PROJECT_ID,
		status: 'failed',
		seed_count: 1,
		fetched_count: 7,
		failed_count: 2,
		queued_count: 0,
		max_depth: 1,
		created_at: '2026-01-10T15:00:00Z',
		started_at: '2026-01-10T15:00:01Z',
		completed_at: '2026-01-10T15:01:00Z'
	}
];

const projection: ProjectionStatusDto = {
	project_id: VISUAL_PROJECT_ID,
	state: 'ready',
	watermark: 42,
	revision: 42,
	lag: 0,
	last_success_at: VISUAL_NOW,
	last_error: null,
	rebuild_state: null
};

const dependencies: DependencyStatus = {
	postgresql: {
		state: 'available',
		lag: null,
		backlog: null,
		oldest_age_seconds: null,
		last_success_at: null,
		recent_failed: null
	},
	worker: {
		state: 'available',
		lag: 0,
		backlog: 0,
		oldest_age_seconds: null,
		last_success_at: null,
		recent_failed: 0
	}
};

const reportDetails: ReportDetailDto = {
	report_id: leadReport.report_id,
	doi: leadReport.doi,
	title: leadReport.title,
	issued_year: leadReport.issued_year,
	type: leadReport.type,
	total_citations: leadReport.total_citations,
	metrics_as_of: leadReport.metrics_as_of,
	metrics_stale: leadReport.metrics_stale,
	abstract:
		'This deterministic abstract gives the article inspector useful content without contacting a provider.',
	container_title: 'Journal of Reproducible Evidence',
	publisher: 'DeepRef Fixture Press',
	published_year: 2024,
	references_count: 36,
	raw: { source: 'visual-fixture', version: 1 },
	url: null
};

const reviewCriteria: EligibilityCriterionDto[] = [
	{
		id: 'visual-criterion',
		label: 'Relevant evidence',
		description: 'Reports describe a reproducible evidence workflow.',
		kind: 'inclusion',
		dimension: 'other',
		stage: 'both',
		ordinal: 0
	}
];

const reviewProtocol: ProtocolDto = {
	id: 'visual-protocol',
	project_id: VISUAL_PROJECT_ID,
	version: 1,
	revision: 1,
	name: 'Evidence mapping protocol',
	objective: 'Assess transparent evidence synthesis workflows.',
	question: 'How can evidence reviews remain reproducible?',
	framework_kind: 'pico',
	framework_fields: {
		population: 'Evidence reviews',
		intervention: 'Transparent methods',
		comparator: 'Opaque workflows',
		outcome: 'Reproducibility'
	},
	status: 'published',
	criteria: reviewCriteria,
	created_at: PROJECT_TIMESTAMP,
	updated_at: VISUAL_NOW,
	published_at: '2026-01-10T12:00:00Z',
	amendment_of: null
};

const protocolVersions: ProtocolVersionsResponse = { items: [reviewProtocol] };

const screeningQueue: ScreeningQueueDto = {
	items: [
		{
			report_id: VISUAL_REPORT_ID,
			title: leadReport.title ?? null,
			abstract_text:
				'An evidence-mapping workflow can make review decisions auditable and reproducible.',
			doi: leadReport.doi ?? null,
			publication_year: leadReport.issued_year ?? null,
			title_abstract_status: 'unscreened',
			full_text_status: 'not_required',
			final_status: 'unscreened',
			revision: 0
		}
	],
	next_cursor: null,
	progress: { total: 1, screened: 0, unscreened: 1, included: 0, excluded: 0, maybe: 0 },
	sort: 'created_asc',
	status: 'unscreened',
	total: 1
};

const fullTextQueue: FullTextQueueDto = {
	items: [
		{
			report_id: VISUAL_REPORT_ID,
			title: leadReport.title ?? null,
			abstract_text: reportDetails.abstract ?? null,
			doi: leadReport.doi ?? null,
			publication_year: leadReport.issued_year ?? null,
			full_text_status: 'unscreened',
			revision: 0,
			document: null
		}
	],
	next_cursor: null
};

const fullTextExclusionReasons: FullTextExclusionReasonDto[] = [
	{ id: 'wrong-design', code: 'wrong-design', label: 'Wrong design', stage: 'full_text' }
];

const missingFullText: MissingFullTextDto[] = [];

const workflowGraph: WorkflowGraphDto = {
	nodes: [
		{
			id: 'visual-trigger',
			type: 'trigger.report_added',
			label: 'When a report is added',
			position: { x: 40, y: 80 },
			config: {}
		},
		{
			id: 'visual-action',
			type: 'action.recompute_metrics',
			label: 'Recompute project metrics',
			position: { x: 320, y: 80 },
			config: {}
		}
	],
	edges: [
		{
			id: 'visual-edge',
			from: { node: 'visual-trigger', port: 'out' },
			to: { node: 'visual-action', port: 'in' }
		}
	]
};

const visualWorkflow: WorkflowDto = {
	id: 'visual-workflow',
	project_id: VISUAL_PROJECT_ID,
	name: 'Project maintenance',
	description: 'Keeps project metrics current when a report is added.',
	alert_status: null,
	created_at: PROJECT_TIMESTAMP,
	updated_at: VISUAL_NOW,
	draft_revision: 1,
	graph: workflowGraph,
	has_unpublished_changes: false,
	last_run_at: '2026-01-14T09:30:02Z',
	next_run_at: null,
	published_version: 1,
	signature_required: false,
	status: 'enabled',
	trigger: 'report_added'
};

const workflows: WorkflowDto[] = [visualWorkflow];

const workflowRuns: WorkflowRunDto[] = [
	{
		id: 'visual-workflow-run',
		workflow_id: 'visual-workflow',
		workflow_name: 'Project maintenance',
		actor: 'visual-user',
		created_at: '2026-01-14T09:30:00Z',
		error: null,
		finished_at: '2026-01-14T09:31:00Z',
		graph: workflowGraph,
		nodes: [],
		review_counts: null,
		started_at: '2026-01-14T09:30:02Z',
		status: 'completed',
		test_mode: false,
		trigger: 'report_added',
		trigger_data: {},
		version: 1
	}
];

const automationCatalog: NodeTypeDto[] = [
	{
		id: 'trigger.report_added',
		label: 'When a report is added',
		category: 'trigger',
		description: 'Starts when a report joins the review.',
		branches: false,
		has_side_effects: false,
		config: [],
		inputs: [],
		outputs: [{ id: 'out', label: 'Report', multiple: false, required: false, type: 'report' }]
	},
	{
		id: 'action.recompute_metrics',
		label: 'Recompute project metrics',
		category: 'action',
		description: 'Refreshes citation counts and rank scores for the project.',
		branches: false,
		has_side_effects: true,
		config: [],
		inputs: [{ id: 'in', label: 'Trigger', multiple: false, required: true, type: 'trigger' }],
		outputs: []
	}
];

const automationTemplates: TemplateDto[] = [
	{
		id: 'refresh-project-metrics',
		name: 'Refresh project metrics',
		description: 'Recompute citation counts and rank scores when a report is added.',
		graph: workflowGraph,
		recipe: 'project_maintenance'
	}
];

const dedupeProposals: DedupeProposalDto[] = [
	{
		id: 'visual-dedupe-proposal',
		project_id: VISUAL_PROJECT_ID,
		record_id: 'visual-record',
		proposal_kind: 'fuzzy',
		status: 'pending',
		revision: 0,
		score: 0.91,
		title_similarity: 0.96,
		year_match: true,
		first_author_similarity: 0.88,
		exact_identifier_match: false,
		conflicting_identifier: false,
		source_title: 'Effects of evidence mapping on review quality',
		source_abstract: 'Source record abstract.',
		source_year: 2024,
		source_authors: { family: 'Smith' },
		source_identifiers: { doi: '10.5555/source' },
		candidate_report_id: VISUAL_REPORT_ID,
		candidate_title: 'Effects of evidence mapping on review quality',
		candidate_year: 2024,
		candidate_authors: { family: 'Smith' },
		candidate_identifiers: { doi: leadReport.doi ?? null },
		metadata: { shortlist: 'visual-fixture', threshold: 0.82 },
		created_at: '2026-01-14T09:30:00Z',
		decided_at: null,
		decision_reason: null,
		reviewer_id: null,
		reviewer_kind: null
	}
];

const aiStatus: AiStatusDto = {
	assistant_available: false,
	budget: null,
	configured: false,
	model: null,
	provider: null,
	suggestions_available: false
};

const aiBudget: AiBudgetDto = {
	exhausted: false,
	monthly_budget_usd: 5,
	remaining_usd: 4.58,
	spent_usd: 0.42
};

const aiActivityOverview: AiActivityOverviewDto = {
	open_conflicts: 0,
	second_review_full_text: 'not_enabled',
	second_review_title_abstract: 'not_enabled',
	to_verify: 0,
	undoable: 0
};

const prisma: PrismaDto = {
	ai_quarantined: 0,
	automation_excluded: 0,
	project_id: VISUAL_PROJECT_ID,
	as_of: null,
	identified_records: 3,
	linked_records: 3,
	duplicates_removed: 0,
	unresolved_records: 0,
	pending_dedupe_proposals: 1,
	source_canonical_reports: 3,
	manually_created_reports: 0,
	screened_records: 1,
	screening_high_watermark: 0,
	title_abstract_excluded: 0,
	title_abstract_pending: 0,
	reports_sought: 1,
	reports_not_retrieved: 0,
	full_text_assessed: 0,
	full_text_pending: 0,
	full_text_excluded: 0,
	full_text_exclusions: [],
	full_text_included: 0,
	included_reports_not_grouped: 0,
	included_studies: 0,
	reconciliation_warnings: []
};

const settings: SettingsDto = {
	crossref_mailto: 'research@example.org',
	default_max_depth: 2,
	max_concurrency: 8,
	rate_limit_per_second: 1,
	retry_attempts: 5,
	metadata_provider: 'crossref',
	citation_provider: 'crossref'
};

function pageOf<T>(items: readonly T[]): { items: T[]; next_cursor: null } {
	return { items: [...items], next_cursor: null };
}

const projectsPage: PaginatedResponseProjectDto = pageOf([project]);
const reportsPage: PaginatedResponseReportDto = pageOf(reports);
const ingestionsPage: PaginatedResponseIngestionDto = pageOf(ingestions);
const dedupeProposalsPage: PaginatedResponseDedupeProposalDto = pageOf(dedupeProposals);
const emptyAcquisitions: PaginatedResponseAcquisitionDto = pageOf([]);
const emptyAiActivity: PaginatedResponseAiActivityDto = pageOf([]);
const emptyAiProposals: PaginatedResponseAiProposalDto = pageOf([]);
const emptyNotifications: PaginatedResponseNotificationDto = pageOf([]);
const emptyStudies: StudyListDto = { items: [], next_cursor: null };
const emptyUngrouped: UngroupedReportDto[] = [];
const noDocuments: DocumentDto[] = [];
const noConversations: AssistantConversationDto[] = [];
const noAppraisalDefinitions: AppraisalDefinitionDto[] = [];
const noExtractionFields: ExtractionFieldDto[] = [];
const noScreeningHistory: ScreeningHistoryDto = {
	items: [],
	project_id: VISUAL_PROJECT_ID,
	report_id: VISUAL_REPORT_ID
};
const unreadNotifications: NotificationUnreadDto = { count: 0, latest_revision: 0 };

const projectPath = `/api/projects/${VISUAL_PROJECT_ID}`;
const reportPath = `${projectPath}/reports/${VISUAL_REPORT_ID}`;

/** Every GET the deterministic workspace answers, keyed by path and exact query. */
export const VISUAL_GET_ENDPOINTS: readonly VisualEndpoint[] = [
	// Workspace shell: projects, ingestions, settings, notifications and dependency health.
	{
		path: '/api/health/dependencies',
		query: `project_id=${VISUAL_PROJECT_ID}`,
		body: dependencies
	},
	{ path: '/api/projects', query: 'limit=50', body: projectsPage },
	{ path: projectPath, query: '', body: project },
	{ path: '/api/ingestions', query: 'limit=50', body: ingestionsPage },
	{ path: '/api/settings', query: '', body: settings },
	{ path: '/api/notifications', query: '', body: emptyNotifications },
	{
		path: '/api/notifications/unread-count',
		query: `project_id=${VISUAL_PROJECT_ID}`,
		body: unreadNotifications
	},
	{ path: '/api/ai/status', query: '', body: aiStatus },
	{ path: '/api/automations/catalog', query: '', body: automationCatalog },
	{ path: '/api/automations/templates', query: '', body: automationTemplates },

	// Plan: protocol, review progress and PRISMA.
	{ path: `${projectPath}/protocol`, query: '', body: reviewProtocol },
	{ path: `${projectPath}/review/protocol`, query: '', body: reviewProtocol },
	{ path: `${projectPath}/review/protocol/versions`, query: '', body: protocolVersions },
	{ path: `${projectPath}/projection`, query: '', body: projection },
	{ path: `${projectPath}/prisma`, query: '', body: prisma },

	// Collect: the corpus, its acquisitions and deduplication.
	{ path: `${projectPath}/reports`, query: 'limit=50', body: reportsPage },
	{ path: `${projectPath}/reports`, query: 'limit=100', body: reportsPage },
	{ path: reportPath, query: '', body: reportDetails },
	{ path: `${reportPath}/screening/history`, query: '', body: noScreeningHistory },
	{ path: `${reportPath}/documents`, query: 'limit=10', body: noDocuments },
	{
		path: `${projectPath}/acquisitions`,
		query: 'strategy=file_import&limit=50',
		body: emptyAcquisitions
	},
	{
		path: `${projectPath}/acquisitions`,
		query: 'strategy=pmid_import&limit=50',
		body: emptyAcquisitions
	},
	{
		path: `${projectPath}/deduplication/proposals`,
		query: 'limit=100&status=pending',
		body: dedupeProposalsPage
	},
	{
		path: `${projectPath}/ai/proposals`,
		query: `status=pending&task_kind=duplicate_candidate_detection&limit=1&target_record_id=visual-record&candidate_report_id=${VISUAL_REPORT_ID}`,
		body: emptyAiProposals
	},

	// Review: screening queues, full text, studies, appraisal and extraction.
	{ path: `${projectPath}/screening`, query: 'status=maybe&limit=1', body: screeningQueue },
	{
		path: `${projectPath}/screening`,
		query: 'status=unscreened&sort=created_asc&limit=25',
		body: screeningQueue
	},
	{ path: `${projectPath}/screening/full-text`, query: 'limit=100', body: fullTextQueue },
	{
		path: `${projectPath}/screening/full-text/missing`,
		query: 'limit=100',
		body: missingFullText
	},
	{
		path: `${projectPath}/screening/full-text/reasons`,
		query: '',
		body: fullTextExclusionReasons
	},
	{ path: `${projectPath}/studies`, query: 'limit=100', body: emptyStudies },
	{ path: `${projectPath}/studies/ungrouped-reports`, query: '', body: emptyUngrouped },
	{ path: `${projectPath}/appraisal-definitions`, query: '', body: noAppraisalDefinitions },
	{ path: `${projectPath}/extraction/fields`, query: '', body: noExtractionFields },

	// Operate: automations, the assistant and the AI activity log.
	{ path: `${projectPath}/automations/workflows`, query: '', body: workflows },
	{
		path: `${projectPath}/automations/workflows/${visualWorkflow.id}`,
		query: '',
		body: visualWorkflow
	},
	{
		path: `${projectPath}/automations/workflow-runs`,
		query: 'limit=100',
		body: workflowRuns
	},
	{ path: `${projectPath}/assistant/conversations`, query: '', body: noConversations },
	{ path: `${projectPath}/ai/budget`, query: '', body: aiBudget },
	{ path: `${projectPath}/ai/activity/overview`, query: '', body: aiActivityOverview },
	{ path: `${projectPath}/ai/activity`, query: 'limit=3&undone=false', body: emptyAiActivity }
];
