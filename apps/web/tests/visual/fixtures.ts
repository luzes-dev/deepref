import { expect, test as base, type Page, type Route } from '@playwright/test';

export const VISUAL_PROJECT_ID = 'visual-project';
export const VISUAL_INGESTION_ID = 'visual-ingestion';
export const VISUAL_REPORT_ID = '00000000-0000-4000-8000-000000000001';
const FIXED_NOW = Date.parse('2026-01-15T12:00:00.000Z');

const project = {
	id: VISUAL_PROJECT_ID,
	name: 'Evidence synthesis workspace',
	description: 'A stable fixture for shell and accessibility review.',
	default_max_depth: 2,
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-15T12:00:00Z'
};

const reports = Array.from({ length: 12 }, (_, index) => ({
	report_id:
		index === 0
			? VISUAL_REPORT_ID
			: `00000000-0000-4000-8000-${String(index + 1).padStart(12, '0')}`,
	doi: `10.5555/deepref-fixture-${String(index + 1).padStart(2, '0')}`,
	title: [
		'Effects of evidence mapping on review quality',
		'Citation networks for transparent synthesis',
		'Human-centred methods for systematic reviews',
		'Reproducible retrieval across scholarly databases'
	][index % 4],
	issued_year: 2020 + (index % 5),
	type: index % 3 === 0 ? 'review' : 'article',
	total_citations: 18 + index * 3,
	internal_citations: index % 6,
	outbound_internal_references: index % 4,
	rank_score: Number((0.96 - index * 0.045).toFixed(3)),
	metrics_as_of: '2026-01-15T12:00:00Z',
	metrics_stale: false
}));

const ingestions = [
	{
		id: VISUAL_INGESTION_ID,
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

const projection = {
	project_id: VISUAL_PROJECT_ID,
	state: 'ready',
	watermark: 42,
	revision: 42,
	lag: 0,
	last_success_at: '2026-01-15T12:00:00Z'
};

const dependencies = {
	postgresql: { state: 'available', lag: null, backlog: null, oldest_age_seconds: null },
	worker: { state: 'available', lag: 0, backlog: 0, oldest_age_seconds: null }
};

const reportDetails = {
	...reports[0],
	abstract:
		'This deterministic abstract gives the article inspector useful content without contacting a provider.',
	container_title: 'Journal of Reproducible Evidence',
	publisher: 'DeepRef Fixture Press',
	published_year: 2024,
	references_count: 36,
	raw: { source: 'visual-fixture', version: 1 },
	url: null
};

const reviewProtocol = {
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
	criteria: [
		{
			id: 'visual-criterion',
			label: 'Relevant evidence',
			description: 'Reports describe a reproducible evidence workflow.',
			kind: 'inclusion',
			dimension: 'other',
			stage: 'both',
			ordinal: 0
		}
	],
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-15T12:00:00Z',
	published_at: '2026-01-10T12:00:00Z',
	amendment_of: null
};

const screeningQueue = {
	items: [
		{
			report_id: VISUAL_REPORT_ID,
			title: reports[0]?.title,
			abstract_text:
				'An evidence-mapping workflow can make review decisions auditable and reproducible.',
			doi: reports[0]?.doi,
			publication_year: reports[0]?.issued_year,
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

const automationDefinitions = [
	{
		id: 'visual-definition',
		project_id: VISUAL_PROJECT_ID,
		name: 'Project maintenance',
		recipe: 'project_maintenance',
		version: 1,
		trigger: 'report_added',
		status: 'active',
		steps: [{ ordinal: 0, key: 'recompute_project_metrics', kind: 'deterministic_action' }],
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-15T12:00:00Z'
	}
];

const automationRuns = [
	{
		id: 'visual-run',
		project_id: VISUAL_PROJECT_ID,
		definition_id: 'visual-definition',
		recipe: 'project_maintenance',
		version: 1,
		trigger: 'report_added',
		trigger_reference: null,
		status: 'completed',
		created_at: '2026-01-14T09:30:00Z',
		started_at: '2026-01-14T09:30:02Z',
		finished_at: '2026-01-14T09:31:00Z',
		error: null,
		job: {
			id: 'visual-job',
			status: 'completed',
			attempts: 1,
			max_attempts: 3,
			available_at: '2026-01-14T09:30:00Z',
			leased_until: null,
			last_error: null
		},
		steps: [
			{
				id: 'visual-step-run',
				ordinal: 0,
				key: 'recompute_project_metrics',
				kind: 'deterministic_action',
				status: 'completed',
				attempts: 1,
				claimed_by: 'visual-worker',
				started_at: '2026-01-14T09:30:02Z',
				finished_at: '2026-01-14T09:31:00Z',
				error: null
			}
		],
		usage: { input_tokens: 12, output_tokens: 34, cost_micros: 0 }
	}
];

const assistantTools = [
	['get_project_protocol', 'read'],
	['search_project_reports', 'read'],
	['propose_screening_decision', 'proposal']
].map(([name, kind]) => ({
	name,
	kind,
	authority_tier: kind === 'read' ? 'read_only' : 'scientific_conclusion',
	description: `${name} fixture tool`
}));

const dedupeProposals = [
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
		candidate_identifiers: { doi: reports[0]?.doi },
		metadata: { shortlist: 'visual-fixture', threshold: 0.82 },
		created_at: '2026-01-14T09:30:00Z'
	}
];

const pageOf = (items: readonly unknown[]) => ({ items, next_cursor: null });

export type VisualApi = {
	readonly requests: readonly string[];
	readonly unhandledRequests: readonly string[];
	install(page: Page): Promise<void>;
};

function jsonResponse(route: Route, body: unknown, status = 200): Promise<void> {
	return route.fulfill({
		status,
		contentType: 'application/json',
		body: JSON.stringify(body) ?? 'null'
	});
}

type EndpointResponse = { body: unknown; status: number };

const GET_ENDPOINT_RESPONSES: ReadonlyMap<string, EndpointResponse> = new Map([
	['/api/health/dependencies', { body: dependencies, status: 200 }],
	['/api/projects', { body: pageOf([project]), status: 200 }],
	[`/api/projects/${VISUAL_PROJECT_ID}`, { body: project, status: 200 }],
	[`/api/projects/${VISUAL_PROJECT_ID}/protocol`, { body: reviewProtocol, status: 200 }],
	[`/api/projects/${VISUAL_PROJECT_ID}/review/protocol`, { body: reviewProtocol, status: 200 }],
	[`/api/projects/${VISUAL_PROJECT_ID}/screening`, { body: screeningQueue, status: 200 }],
	[
		`/api/projects/${VISUAL_PROJECT_ID}/reports/${VISUAL_REPORT_ID}/screening/history`,
		{
			body: {
				project_id: VISUAL_PROJECT_ID,
				report_id: VISUAL_REPORT_ID,
				items: []
			},
			status: 200
		}
	],
	[`/api/projects/${VISUAL_PROJECT_ID}/reports`, { body: pageOf(reports), status: 200 }],
	[
		`/api/projects/${VISUAL_PROJECT_ID}/reports/${VISUAL_REPORT_ID}`,
		{ body: reportDetails, status: 200 }
	],
	[`/api/projects/${VISUAL_PROJECT_ID}/projection`, { body: projection, status: 200 }],
	[
		`/api/projects/${VISUAL_PROJECT_ID}/graph`,
		{
			body: {
				nodes: reports.map((report) => ({ ...report, id: report.report_id })),
				edges: reports.slice(1).map((report, index) => ({
					source: reports[index]?.report_id,
					target: report.report_id
				})),
				projection,
				truncated: false
			},
			status: 200
		}
	],
	[
		`/api/projects/${VISUAL_PROJECT_ID}/recommendations`,
		{
			body: {
				foundational: [],
				core_to_project: [],
				underexplored: [],
				projection
			},
			status: 200
		}
	],
	[
		`/api/projects/${VISUAL_PROJECT_ID}/screening/full-text`,
		{
			body: {
				items: [
					{
						report_id: VISUAL_REPORT_ID,
						title: reports[0]?.title,
						abstract_text: reportDetails.abstract,
						doi: reports[0]?.doi,
						publication_year: reports[0]?.issued_year,
						full_text_status: 'unscreened',
						revision: 0,
						document: null
					}
				],
				next_cursor: null
			},
			status: 200
		}
	],
	[`/api/projects/${VISUAL_PROJECT_ID}/screening/full-text/missing`, { body: [], status: 200 }],
	[
		`/api/projects/${VISUAL_PROJECT_ID}/screening/full-text/reasons`,
		{
			body: [
				{
					id: 'wrong-design',
					code: 'wrong-design',
					label: 'Wrong design',
					stage: 'full_text'
				}
			],
			status: 200
		}
	],
	[
		`/api/projects/${VISUAL_PROJECT_ID}/reports/${VISUAL_REPORT_ID}/documents`,
		{ body: [], status: 200 }
	],
	[`/api/projects/${VISUAL_PROJECT_ID}/appraisal-definitions`, { body: [], status: 200 }],
	[`/api/projects/${VISUAL_PROJECT_ID}/studies`, { body: pageOf([]), status: 200 }],
	[`/api/projects/${VISUAL_PROJECT_ID}/extraction/fields`, { body: [], status: 200 }],
	[
		`/api/projects/${VISUAL_PROJECT_ID}/automations/definitions`,
		{ body: automationDefinitions, status: 200 }
	],
	[`/api/projects/${VISUAL_PROJECT_ID}/automations/runs`, { body: automationRuns, status: 200 }],
	[`/api/projects/${VISUAL_PROJECT_ID}/assistant/tools`, { body: assistantTools, status: 200 }],
	[
		`/api/projects/${VISUAL_PROJECT_ID}/deduplication/proposals`,
		{ body: pageOf(dedupeProposals), status: 200 }
	],
	[`/api/projects/${VISUAL_PROJECT_ID}/ai/proposals`, { body: pageOf([]), status: 200 }],
	['/api/ingestions', { body: pageOf(ingestions), status: 200 }],
	[`/api/ingestions/${VISUAL_INGESTION_ID}`, { body: ingestions[0], status: 200 }],
	[
		`/api/ingestions/${VISUAL_INGESTION_ID}/items`,
		{
			body: pageOf([
				{
					id: 'visual-ingestion-item',
					ingestion_id: VISUAL_INGESTION_ID,
					doi: reports[0]?.doi,
					status: 'fetched',
					error: null
				}
			]),
			status: 200
		}
	],
	[
		'/api/settings',
		{
			body: {
				crossref_mailto: 'research@example.org',
				default_max_depth: 2,
				max_concurrency: 8,
				rate_limit_per_second: 1,
				retry_attempts: 5,
				metadata_provider: 'crossref',
				citation_provider: 'crossref'
			},
			status: 200
		}
	]
]);

function endpointResponse(pathname: string, method: string): EndpointResponse {
	if (method !== 'GET') {
		return {
			body: { detail: `No visual fixture for ${method} ${pathname}` },
			status: 404
		};
	}
	return (
		GET_ENDPOINT_RESPONSES.get(pathname) ?? {
			body: { detail: `No visual fixture for ${method} ${pathname}` },
			status: 404
		}
	);
}

async function installDeterminism(page: Page): Promise<void> {
	await page.addInitScript(
		({ fixedNow }) => {
			Date.now = () => fixedNow;
			let uuidCounter = 0;
			try {
				Object.defineProperty(globalThis.crypto, 'randomUUID', {
					configurable: true,
					value: () => {
						uuidCounter += 1;
						return `00000000-0000-4000-8000-${String(uuidCounter).padStart(12, '0')}`;
					}
				});
			} catch {
				// Some browser versions expose a non-configurable Crypto object. The fixture
				// does not rely on UUID generation, so determinism remains intact.
			}
		},
		{ fixedNow: FIXED_NOW }
	);
}

async function installMotionReset(page: Page): Promise<void> {
	await page.addStyleTag({
		content: `
			*, *::before, *::after {
				animation-delay: 0s !important;
				animation-duration: 0s !important;
				animation-iteration-count: 1 !important;
				transition-delay: 0s !important;
				transition-duration: 0s !important;
				scroll-behavior: auto !important;
			}
		`
	});
}

export async function settleVisualPage(page: Page): Promise<void> {
	await page.waitForLoadState('networkidle');
	await page.evaluate(async () => {
		await document.fonts.ready;
		await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
	});
}

function createVisualApi(): VisualApi {
	const requests: string[] = [];
	const unhandledRequests: string[] = [];

	return {
		get requests() {
			return [...requests];
		},
		get unhandledRequests() {
			return [...unhandledRequests];
		},
		async install(page: Page) {
			await installDeterminism(page);
			await page.emulateMedia({ reducedMotion: 'reduce' });
			await page.route('**/api/**', async (route) => {
				const request = route.request();
				const url = new URL(request.url());
				const method = request.method();
				const requestLabel = `${method} ${url.pathname}${url.search}`;
				requests.push(requestLabel);
				const response = endpointResponse(url.pathname, method);
				if (response.status === 404) unhandledRequests.push(requestLabel);
				await jsonResponse(route, response.body, response.status);
			});
			await page.addInitScript(() => {
				const applyColorScheme = () => {
					const root = document.documentElement;
					if (root) {
						root.classList.toggle(
							'dark',
							window.matchMedia('(prefers-color-scheme: dark)').matches
						);
					}
				};
				if (document.readyState === 'loading') {
					document.addEventListener('DOMContentLoaded', applyColorScheme, { once: true });
				} else {
					applyColorScheme();
				}
			});
			await page.goto('/projects/visual-project/overview');
			await settleVisualPage(page);
			await installMotionReset(page);
		}
	};
}

type VisualFixtures = {
	visualApi: VisualApi;
};

export const test = base.extend<VisualFixtures>({
	visualApi: [
		async ({ page }, use) => {
			const visualApi = createVisualApi();
			await visualApi.install(page);
			await use(visualApi);
			expect(visualApi.unhandledRequests).toEqual([]);
		},
		{ auto: true }
	]
});

export { expect } from '@playwright/test';

export async function captureViewport(page: Page, snapshotName: string): Promise<void> {
	await settleVisualPage(page);
	await expect(page).toHaveScreenshot(snapshotName, {
		animations: 'disabled',
		caret: 'hide'
	});
}

export async function captureDarkViewport(page: Page, snapshotName: string): Promise<void> {
	if (!test.info().project.name.includes('-dark-')) return;
	await captureViewport(page, snapshotName);
}

export async function isMobileViewport(page: Page): Promise<boolean> {
	return page.evaluate(() => window.innerWidth < 768);
}
