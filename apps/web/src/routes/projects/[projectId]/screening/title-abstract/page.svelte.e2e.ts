import { expect, test, type Page } from '@playwright/test';

const api = 'http://localhost:4173/api';
const projectId = 'project-1';

const project = {
	id: projectId,
	name: 'Screening project',
	description: 'A mocked project',
	default_max_depth: 2,
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z'
};

const dependencies = {
	postgresql: { state: 'available', lag: null, backlog: null, oldest_age_seconds: null },
	worker: { state: 'available', lag: 0, backlog: 0, oldest_age_seconds: null }
};

type Report = {
	report_id: string;
	title: string;
	abstract_text: string;
	doi: string;
	publication_year: number;
	title_abstract_status: string;
	full_text_status: string;
	final_status: string;
	revision: number;
};

type HistoryItem = {
	id: string;
	event_kind: 'decision' | 'undo';
	stage: 'title_abstract';
	decision: string | null;
	notes: string | null;
	protocol_version_id: string;
	actor_kind: string;
	actor_id: string;
	supersedes_event_id: string | null;
	undoes_event_id: string | null;
	created_at: string;
	previous_title_abstract_status: string;
	previous_full_text_status: string;
	previous_full_text_exclusion_reason_id: string | null;
	previous_final_status: string;
	result_title_abstract_status: string;
	result_full_text_status: string;
	result_full_text_exclusion_reason_id: string | null;
	result_final_status: string;
};

type ScreeningMock = {
	reports: Report[];
	history: Map<string, HistoryItem[]>;
	conflict: boolean;
	decisionExpectedRevisions: number[];
	queueRequests: string[];
	undoDelayMs: number;
	undoCompleted: boolean;
};

function makeReports(count = 4): Report[] {
	return Array.from({ length: count }, (_, index) => ({
		report_id: `report-${index + 1}`,
		title: `Screening report ${String(index + 1).padStart(2, '0')}`,
		abstract_text: `Abstract for report ${index + 1}.`,
		doi: `10.5555/report-${index + 1}`,
		publication_year: 2020 + (index % 5),
		title_abstract_status: 'unscreened',
		full_text_status: 'not_required',
		final_status: 'unscreened',
		revision: 0
	}));
}

function protocol() {
	return {
		id: 'protocol-1',
		project_id: projectId,
		version: 3,
		revision: 1,
		name: 'Default evidence screening protocol',
		objective: 'Identify relevant evidence',
		question: 'Does this report answer the review question?',
		framework_kind: 'pico',
		framework_fields: {},
		status: 'published',
		criteria: [
			{
				id: 'population',
				label: 'Population',
				description: 'Matches the review population.',
				kind: 'inclusion',
				dimension: 'population',
				stage: 'title_abstract',
				ordinal: 1
			}
		],
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-01T00:00:00Z',
		published_at: '2026-01-01T00:00:00Z',
		amendment_of: null
	};
}

function stateFor(report: Report) {
	return {
		project_id: projectId,
		report_id: report.report_id,
		title_abstract_status: report.title_abstract_status,
		full_text_status: report.full_text_status,
		full_text_exclusion_reason_id: null,
		final_status: report.final_status,
		revision: report.revision,
		last_event_id: null,
		updated_at: '2026-01-01T00:00:00Z'
	};
}

function historyItem(
	report: Report,
	previousStatus: string,
	resultStatus: string,
	eventKind: 'decision' | 'undo',
	index: number,
	undoesEventId: string | null = null
): HistoryItem {
	const id = `${report.report_id}-event-${index}`;
	return {
		id,
		event_kind: eventKind,
		stage: 'title_abstract',
		decision: eventKind === 'decision' ? resultStatus : null,
		notes: null,
		protocol_version_id: 'protocol-1',
		actor_kind: 'user',
		actor_id: 'e2e-user',
		supersedes_event_id: index > 1 ? `${report.report_id}-event-${index - 1}` : null,
		undoes_event_id: undoesEventId,
		created_at: `2026-01-01T00:00:0${index}Z`,
		previous_title_abstract_status: previousStatus,
		previous_full_text_status: 'not_required',
		previous_full_text_exclusion_reason_id: null,
		previous_final_status: previousStatus,
		result_title_abstract_status: resultStatus,
		result_full_text_status: 'not_required',
		result_full_text_exclusion_reason_id: null,
		result_final_status: resultStatus
	};
}

async function installMocks(page: Page, mock: ScreeningMock) {
	await page.route(`${api}/health/dependencies`, (route) =>
		route.fulfill({ json: dependencies })
	);
	await page.route(/\/api\/projects(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [project], next_cursor: null } })
	);
	await page.route(`${api}/projects/${projectId}`, (route) => route.fulfill({ json: project }));
	await page.route(`${api}/projects/${projectId}/protocol`, (route) =>
		route.fulfill({ json: protocol() })
	);
	await page.route(/\/api\/projects\/project-1\/reports(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [], next_cursor: null } })
	);
	await page.route(/\/api\/ingestions(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [], next_cursor: null } })
	);
	await page.route(
		new RegExp(`/api/projects/${projectId}/screening(?:\\?.*)?$`),
		async (route) => {
			const url = new URL(route.request().url());
			const status = url.searchParams.get('status') ?? 'unscreened';
			const cursor = url.searchParams.get('cursor');
			const search = (url.searchParams.get('search') ?? '').toLowerCase();
			const filtered = mock.reports.filter(
				(report) =>
					(status === 'all' || report.title_abstract_status === status) &&
					(!search ||
						`${report.title} ${report.abstract_text}`.toLowerCase().includes(search))
			);
			const start = cursor ? Number(cursor.replace('page-', '')) : 0;
			const pageSize = Number(url.searchParams.get('limit') ?? 25);
			const items = filtered.slice(start, start + pageSize);
			mock.queueRequests.push(url.search);
			const progress = {
				total: mock.reports.length,
				screened: 0,
				unscreened: 0,
				included: 0,
				excluded: 0,
				maybe: 0
			};
			for (const report of mock.reports) {
				if (report.title_abstract_status === 'unscreened') progress.unscreened += 1;
				else {
					progress.screened += 1;
					const key = { include: 'included', exclude: 'excluded', maybe: 'maybe' }[
						report.title_abstract_status
					] as 'included' | 'excluded' | 'maybe';
					progress[key] += 1;
				}
			}
			await route.fulfill({
				json: {
					items,
					status,
					sort: url.searchParams.get('sort') ?? 'created_asc',
					total: filtered.length,
					next_cursor:
						start + pageSize < filtered.length ? `page-${start + pageSize}` : null,
					progress
				}
			});
		}
	);
	await page.route(
		new RegExp(`/api/projects/${projectId}/reports/([^/]+)/screening/history$`),
		async (route) => {
			const reportId = route.request().url().split('/reports/')[1].split('/')[0];
			await route.fulfill({
				json: {
					project_id: projectId,
					report_id: reportId,
					items: mock.history.get(reportId) ?? []
				}
			});
		}
	);
	await page.route(
		new RegExp(`/api/projects/${projectId}/reports/([^/]+)/screening$`),
		async (route) => {
			if (route.request().method() !== 'POST') return route.continue();
			const reportId = route.request().url().split('/reports/')[1].split('/')[0];
			const report = mock.reports.find((candidate) => candidate.report_id === reportId);
			if (!report)
				return route.fulfill({
					status: 404,
					json: { code: 'not_found', message: 'missing' }
				});
			const body = route.request().postDataJSON() as {
				decision: string;
				expected_revision: number;
			};
			mock.decisionExpectedRevisions.push(body.expected_revision);
			if (mock.conflict) {
				mock.conflict = false;
				report.title_abstract_status = 'include';
				report.final_status = 'pending_full_text';
				report.revision = 1;
				mock.history.set(reportId, [
					historyItem(report, 'unscreened', 'include', 'decision', 1)
				]);
				return route.fulfill({
					status: 409,
					json: {
						code: 'screening_revision_conflict',
						message: 'stale screening revision',
						details: { currentRevision: 1, currentState: stateFor(report) }
					}
				});
			}
			const previous = report.title_abstract_status;
			report.title_abstract_status = body.decision;
			report.final_status = body.decision === 'include' ? 'pending_full_text' : body.decision;
			report.revision += 1;
			const history = mock.history.get(reportId) ?? [];
			history.push(
				historyItem(report, previous, body.decision, 'decision', history.length + 1)
			);
			mock.history.set(reportId, history);
			await route.fulfill({ json: stateFor(report) });
		}
	);
	await page.route(
		new RegExp(`/api/projects/${projectId}/reports/([^/]+)/screening/undo$`),
		async (route) => {
			const reportId = route.request().url().split('/reports/')[1].split('/')[0];
			const report = mock.reports.find((candidate) => candidate.report_id === reportId);
			if (!report)
				return route.fulfill({
					status: 404,
					json: { code: 'not_found', message: 'missing' }
				});
			const body = route.request().postDataJSON() as { expected_revision: number };
			const history = mock.history.get(reportId) ?? [];
			if (body.expected_revision !== report.revision || history.length === 0) {
				return route.fulfill({
					status: 409,
					json: { code: 'screening_revision_conflict', message: 'stale' }
				});
			}
			const latest = history.at(-1)!;
			const previous = report.title_abstract_status;
			report.title_abstract_status = latest.previous_title_abstract_status;
			report.final_status = report.title_abstract_status;
			report.full_text_status = 'not_required';
			report.revision += 1;
			history.push(
				historyItem(
					report,
					previous,
					report.title_abstract_status,
					'undo',
					history.length + 1,
					latest.id
				)
			);
			if (mock.undoDelayMs)
				await new Promise((resolve) => setTimeout(resolve, mock.undoDelayMs));
			mock.undoCompleted = true;
			await route.fulfill({ json: stateFor(report) });
		}
	);
}

async function setup(
	page: Page,
	options: { count?: number; conflict?: boolean; undoDelayMs?: number } = {}
) {
	const mock: ScreeningMock = {
		reports: makeReports(options.count ?? 4),
		history: new Map(),
		conflict: options.conflict ?? false,
		decisionExpectedRevisions: [],
		queueRequests: [],
		undoDelayMs: options.undoDelayMs ?? 0,
		undoCompleted: false
	};
	await installMocks(page, mock);
	return mock;
}

test('decides A, advances to B, then U restores A immediately and persists history', async ({
	page
}) => {
	const mock = await setup(page, { undoDelayMs: 500 });
	await page.goto(`/projects/${projectId}/screening/title-abstract`);
	await expect(page.getByRole('heading', { name: 'Screening report 01' })).toBeVisible();

	await page.getByRole('button', { name: /Include/ }).click();
	await expect(page.getByRole('heading', { name: 'Screening report 02' })).toBeVisible();
	await page.evaluate(() => {
		document.body.tabIndex = -1;
		document.body.focus();
	});
	await page.keyboard.press('u');
	await expect(page.getByRole('heading', { name: 'Screening report 01' })).toBeVisible();
	await expect.poll(() => mock.undoCompleted).toBe(true);
	await expect(page.getByText('Undo', { exact: true })).toBeVisible();
	await expect(page.getByText(/Undoes/)).toBeVisible();
	expect(mock.reports[0].title_abstract_status).toBe('unscreened');
	expect(mock.reports[0].revision).toBe(2);

	await page.reload();
	await expect(page.getByRole('heading', { name: 'Screening report 01' })).toBeVisible();
});

test('guards shortcuts in controls and an actually open overlay', async ({ page }) => {
	const mock = await setup(page);
	await page.goto(`/projects/${projectId}/screening/title-abstract`);
	await page.getByLabel('Search title or abstract').focus();
	await page.keyboard.press('i');
	expect(mock.reports[0].revision).toBe(0);

	await page.evaluate(() => {
		const overlay = document.createElement('div');
		overlay.setAttribute('role', 'dialog');
		overlay.dataset.state = 'open';
		document.body.append(overlay);
		document.body.tabIndex = -1;
		document.body.focus();
	});
	await page.keyboard.press('i');
	expect(mock.reports[0].revision).toBe(0);
});

test('renders virtual rows and fetches the second cursor page', async ({ page }) => {
	const mock = await setup(page, { count: 60 });
	await page.goto(
		`/projects/${projectId}/screening/title-abstract?mode=table&status=all&sort=title_asc`
	);
	await expect(page.getByRole('row').first()).toBeVisible();
	await page.getByRole('table').evaluate((element) => {
		element.scrollTop = element.scrollHeight;
		element.dispatchEvent(new Event('scroll'));
	});
	await expect
		.poll(() => mock.queueRequests.some((request) => request.includes('cursor=page-25')))
		.toBe(true);
	await expect(page.getByRole('row', { name: /Screening report 30/ })).toBeVisible();
});

test('preserves mode, status, sort, and report through refresh and history navigation', async ({
	page
}) => {
	await setup(page, { count: 8 });
	const url = `/projects/${projectId}/screening/title-abstract?mode=table&status=all&sort=title_desc&report=report-2`;
	await page.goto(url);
	await expect(page.getByText('Table mode')).toBeVisible();
	await page.reload();
	await expect(page).toHaveURL(/mode=table.*status=all.*sort=title_desc.*report=report-2/);
	await page.getByRole('button', { name: /Focus/ }).click();
	await expect(page).toHaveURL(/status=all.*sort=title_desc.*report=report-2/);
	await expect(page.getByText('Focus mode')).toBeVisible();
	await page.goBack();
	await expect(page).toHaveURL(/mode=table.*status=all.*sort=title_desc.*report=report-2/);
	await page.goForward();
	await expect(page).toHaveURL(/status=all.*sort=title_desc.*report=report-2/);
	await expect(page.getByText('Focus mode')).toBeVisible();
});

test('reconciles a 409 with authoritative state and sends its revision next', async ({ page }) => {
	const mock = await setup(page, { conflict: true });
	await page.goto(`/projects/${projectId}/screening/title-abstract`);
	await page.getByRole('button', { name: /Include/ }).click();
	await expect(page.getByRole('status')).toContainText('authoritative server state');
	await expect(page.locator('#screening-status')).toHaveValue('all');
	await expect(page.getByRole('heading', { name: 'Screening report 01' })).toBeVisible();
	await page.getByRole('button', { name: /Maybe/ }).click();
	await expect.poll(() => mock.decisionExpectedRevisions.at(-1)).toBe(1);
	await expect(page.getByRole('heading', { name: 'Screening report 02' })).toBeVisible();
});

test('reviews a deterministic title and abstract AI proposal before applying it', async ({
	page
}) => {
	await setup(page, { count: 1 });
	const proposal = {
		id: 'ai-proposal-1',
		project_id: projectId,
		task_kind: 'title_abstract_screening',
		status: 'pending',
		target_report_id: 'report-1',
		target_record_id: null,
		protocol_version_id: 'protocol-1',
		expected_revision: 0,
		provider: 'deterministic-fixture',
		model: 'fixture-model',
		model_version: 'fixture-v1',
		prompt_version: 'screening.v1',
		schema_version: 'screening.schema.v1',
		model_run_id: 'run-1',
		operation: 'screening_suggestion',
		entity_type: 'report',
		entity_id: 'report-1',
		authority_tier: 'scientific_conclusion',
		created_at: '2026-01-01T00:00:00Z',
		payload: {
			kind: 'screening',
			task_kind: 'title_abstract_screening',
			report_id: 'report-1',
			expected_revision: 0,
			stage: 'title_abstract',
			protocol_version_id: 'protocol-1',
			criteria: [
				{
					criterion_id: 'population',
					criterion_label: 'Population',
					judgment: 'unclear',
					rationale: 'The fixture abstains when the abstract is insufficient.',
					evidence: [
						{
							kind: 'report_metadata',
							report_id: 'report-1',
							field: 'title',
							content_hash: 'a'.repeat(64)
						}
					]
				}
			],
			suggested_decision: { kind: 'maybe' },
			uncertainties: ['Abstract does not identify the target population.']
		}
	};
	let pending = false;
	const decisions: Array<Record<string, unknown>> = [];
	await page.route(
		new RegExp(`/api/projects/${projectId}/ai/proposals(?:\\?.*)?$`),
		async (route) => {
			const url = new URL(route.request().url());
			expect(url.searchParams.get('target_report_id')).toBe('report-1');
			await route.fulfill({ json: { items: pending ? [proposal] : [], next_cursor: null } });
		}
	);
	await page.route(
		new RegExp(`/api/projects/${projectId}/reports/report-1/ai/screening$`),
		async (route) => {
			expect(route.request().method()).toBe('POST');
			pending = true;
			await route.fulfill({ status: 200, json: proposal });
		}
	);
	await page.route(
		new RegExp(`/api/projects/${projectId}/ai/proposals/${proposal.id}/decision$`),
		async (route) => {
			expect(route.request().method()).toBe('POST');
			decisions.push(route.request().postDataJSON());
			pending = false;
			await route.fulfill({
				status: 200,
				json: { data: { ...proposal, status: 'accepted' } }
			});
		}
	);

	await page.goto(`/projects/${projectId}/screening/title-abstract`);
	const ai = page.getByTestId('ai-proposal-review');
	await ai.getByRole('button', { name: 'Request suggestion' }).click();
	await expect(ai.getByText('Population', { exact: true })).toBeVisible();
	await expect(ai.getByText('maybe', { exact: true })).toBeVisible();
	await expect(ai.getByText(/Report metadata.*hash a{12}/)).toBeVisible();
	await expect(ai.getByText('Abstract does not identify the target population.')).toBeVisible();
	await ai.getByRole('button', { name: 'Approve and apply' }).click();
	await expect(ai.getByText('No pending suggestion', { exact: true })).toBeVisible();
	await expect.poll(() => decisions.length).toBe(1);
	expect(decisions[0]).toMatchObject({ decision: 'accept' });
});
