import { expect, test, type Page } from '@playwright/test';

const api = 'http://localhost:4173/api';
const project = {
	id: 'project-1',
	name: 'Assistant project',
	description: 'A mocked project',
	default_max_depth: 2,
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z'
};
const dependencies = {
	postgresql: { state: 'available', lag: null, backlog: null, oldest_age_seconds: null },
	worker: { state: 'available', lag: 0, backlog: 0, oldest_age_seconds: null }
};

const reportId = '11111111-1111-4111-8111-111111111111';
const documentId = '22222222-2222-4222-8222-222222222222';
const blockId = '33333333-3333-4333-8333-333333333333';
const secondBlockId = '44444444-4444-4444-8444-444444444444';
const studyId = '77777777-7777-4777-8777-777777777777';
const recordId = '55555555-5555-4555-8555-555555555555';
const candidateReportId = '66666666-6666-4666-8666-666666666666';

function completedReviewRun(runId: string, proposalId: string) {
	return {
		id: runId,
		project_id: 'project-1',
		definition: 'duplicate_detection',
		subject: {},
		origin: { kind: 'reviewer_requested' },
		state: { kind: 'completed', proposal_id: proposalId },
		created_at: '2026-01-01T00:00:00Z',
		started_at: '2026-01-01T00:00:01Z',
		finished_at: '2026-01-01T00:00:02Z'
	};
}

const catalog = [
	['get_project_protocol', 'read'],
	['get_report', 'read'],
	['read_document_blocks', 'read'],
	['search_document', 'read'],
	['search_project_reports', 'read'],
	['get_screening_state', 'read'],
	['get_study', 'read'],
	['get_appraisal', 'read'],
	['propose_screening_decision', 'proposal'],
	['propose_duplicate_merge', 'proposal'],
	['propose_study_grouping', 'proposal'],
	['propose_classification', 'proposal'],
	['propose_extraction', 'proposal'],
	['propose_appraisal_answer', 'proposal']
].map(([name, kind]) => ({
	name,
	kind,
	authority_tier: kind === 'read' ? 'read_only' : 'scientific_conclusion',
	description: `${name} server description`
}));

async function mockProjectShell(page: Page): Promise<void> {
	await page.route(`${api}/health/dependencies`, (route) =>
		route.fulfill({ json: dependencies })
	);
	await page.route(/\/api\/projects(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [project], next_cursor: null } })
	);
	await page.route(`${api}/projects/project-1`, (route) => route.fulfill({ json: project }));
	await page.route(/\/api\/projects\/project-1\/reports(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [], next_cursor: null } })
	);
	await page.route(/\/api\/ingestions(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [], next_cursor: null } })
	);
}

async function mockAssistantCatalog(page: Page, extraCatalog: Array<Record<string, unknown>> = []) {
	await page.route(`${api}/projects/project-1/assistant/tools`, async (route) => {
		expect(route.request().method()).toBe('GET');
		await route.fulfill({ json: [...catalog, ...extraCatalog] });
	});
}

async function openAssistant(page: Page): Promise<void> {
	await mockProjectShell(page);
	await mockAssistantCatalog(page);
	await page.goto('/projects/project-1/assistant');
	await expect(page.getByRole('heading', { name: 'Project Assistant' })).toBeVisible();
}

test('executes typed read arguments with injected project scope and renders bounded JSON', async ({
	page
}) => {
	await openAssistant(page);
	const requests: Array<Record<string, unknown>> = [];
	await page.route(`${api}/projects/project-1/assistant/tools/execute`, async (route) => {
		expect(route.request().method()).toBe('POST');
		expect(route.request().headers()['x-actor-kind']).toBe('user');
		expect(route.request().headers()['x-actor-id']).toBe('local-user');
		const body = route.request().postDataJSON();
		requests.push(body);
		expect(body).toEqual({
			tool: 'read_document_blocks',
			args: {
				project_id: 'project-1',
				document_id: documentId,
				block_ids: [blockId, secondBlockId]
			}
		});
		await route.fulfill({
			json: {
				kind: 'read',
				data: {
					document_id: documentId,
					blocks: [{ id: blockId, text: 'bounded evidence' }]
				}
			}
		});
	});

	await page.getByTestId('assistant-tool-read_document_blocks').click();
	await page.getByTestId('assistant-field-document_id').fill(documentId);
	await page.getByTestId('assistant-field-block_ids').fill(`${blockId}\n${secondBlockId}`);
	await page.getByTestId('assistant-execute').click();

	await expect(page.getByTestId('assistant-read-result')).toContainText('bounded evidence');
	expect(requests).toHaveLength(1);
});

test('executes a proposal once, shows the receipt, and links to human review', async ({ page }) => {
	await openAssistant(page);
	let request: Record<string, unknown> | undefined;
	await page.route(`${api}/projects/project-1/assistant/tools/execute`, async (route) => {
		request = route.request().postDataJSON();
		await route.fulfill({
			json: {
				kind: 'review_run',
				review_run_id: 'run-123',
				status_path: '/projects/project-1/review-runs/run-123'
			}
		});
	});
	await page.route(`${api}/projects/project-1/review-runs/run-123`, (route) =>
		route.fulfill({ json: completedReviewRun('run-123', 'proposal-123') })
	);

	await page.getByTestId('assistant-tool-propose_duplicate_merge').click();
	await page.getByTestId('assistant-field-source_record_id').fill(recordId);
	await page.getByTestId('assistant-field-candidate_report_id').fill(candidateReportId);
	await page.getByTestId('assistant-execute').click();

	await expect(page.getByTestId('assistant-proposal-receipt')).toContainText('proposal-123');
	await expect(page.getByTestId('assistant-review-link')).toHaveAttribute(
		'href',
		'/projects/project-1/discovery/duplicates'
	);
	expect(request).toEqual({
		tool: 'propose_duplicate_merge',
		args: {
			project_id: 'project-1',
			source_record_id: recordId,
			candidate_report_id: candidateReportId
		}
	});
});

test('links classification proposals to the selected study review', async ({ page }) => {
	await openAssistant(page);
	let request: Record<string, unknown> | undefined;
	await page.route(`${api}/projects/project-1/assistant/tools/execute`, async (route) => {
		request = route.request().postDataJSON();
		await route.fulfill({
			json: {
				kind: 'review_run',
				review_run_id: 'classification-run',
				status_path: '/projects/project-1/review-runs/classification-run'
			}
		});
	});
	await page.route(`${api}/projects/project-1/review-runs/classification-run`, (route) =>
		route.fulfill({ json: completedReviewRun('classification-run', 'classification-proposal') })
	);

	await page.getByTestId('assistant-tool-propose_classification').click();
	await page.getByTestId('assistant-field-study_id').fill(studyId);
	await page.getByTestId('assistant-execute').click();

	await expect(page.getByTestId('assistant-review-link')).toHaveAttribute(
		'href',
		`/projects/project-1/studies?study=${studyId}`
	);
	expect(request).toEqual({
		tool: 'propose_classification',
		args: { project_id: 'project-1', study_id: studyId }
	});
});

test('does not send invalid UUID, block-list, or limit values and ignores unknown server tools', async ({
	page
}) => {
	await mockProjectShell(page);
	await mockAssistantCatalog(page, [
		{
			name: 'future_tool',
			kind: 'read',
			authority_tier: 'read_only',
			description: 'Future server capability'
		}
	]);
	await page.goto('/projects/project-1/assistant');
	await expect(page.getByTestId('assistant-unsupported-tools')).toContainText('future_tool');
	await expect(page.getByTestId('assistant-tool-future_tool')).toHaveCount(0);

	let executeCount = 0;
	await page.route(`${api}/projects/project-1/assistant/tools/execute`, async (route) => {
		executeCount += 1;
		await route.fulfill({ json: { kind: 'read', data: {} } });
	});

	await page.getByTestId('assistant-tool-get_report').click();
	await page.getByTestId('assistant-field-report_id').fill('bad-uuid');
	await expect(page.getByTestId('assistant-execute')).toBeDisabled();

	await page.getByTestId('assistant-tool-read_document_blocks').click();
	await page.getByTestId('assistant-field-document_id').fill(documentId);
	await page.getByTestId('assistant-field-block_ids').fill(`${blockId}\nnot-a-uuid`);
	await expect(page.getByTestId('assistant-execute')).toBeDisabled();

	await page.getByTestId('assistant-tool-search_project_reports').click();
	await page.getByTestId('assistant-field-query').fill('trial');
	await page.getByTestId('assistant-field-limit').fill('101');
	await expect(page.getByTestId('assistant-execute')).toBeDisabled();
	await expect(executeCount).toBe(0);
});

test('shows API permission and provider errors, with an explicit provider retry', async ({
	page
}) => {
	await openAssistant(page);
	let mode: 'forbidden' | 'unavailable' | 'success' = 'forbidden';
	let requests = 0;
	await page.route(`${api}/projects/project-1/assistant/tools/execute`, async (route) => {
		requests += 1;
		if (mode === 'forbidden') {
			await route.fulfill({ status: 403, json: { message: 'assistant access is disabled' } });
		} else if (mode === 'unavailable') {
			mode = 'success';
			await route.fulfill({
				status: 503,
				json: { message: 'provider temporarily unavailable' }
			});
		} else {
			await route.fulfill({
				json: {
					kind: 'review_run',
					review_run_id: 'retried-run',
					status_path: '/projects/project-1/review-runs/retried-run'
				}
			});
		}
	});
	await page.route(`${api}/projects/project-1/review-runs/retried-run`, (route) =>
		route.fulfill({ json: completedReviewRun('retried-run', 'retried-proposal') })
	);

	await page.getByTestId('assistant-tool-propose_screening_decision').click();
	await page.getByTestId('assistant-field-report_id').fill(reportId);
	await page.getByTestId('assistant-execute').click();
	await expect(page.getByTestId('assistant-execution-error')).toContainText(
		'assistant access is disabled'
	);
	await expect(page.getByTestId('assistant-execution-retry')).toHaveCount(0);

	mode = 'unavailable';
	await page.getByTestId('assistant-execute').click();
	await expect(page.getByTestId('assistant-execution-error')).toContainText(
		'provider temporarily unavailable'
	);
	await page.getByTestId('assistant-execution-retry').click();
	await expect(page.getByTestId('assistant-proposal-receipt')).toContainText('retried-proposal');
	expect(requests).toBe(3);
});
