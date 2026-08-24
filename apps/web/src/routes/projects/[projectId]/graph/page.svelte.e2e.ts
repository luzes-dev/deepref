import { expect, test, type Page } from '@playwright/test';

const projectId = 'project-1';
const api = 'http://localhost:4173/api';
const project = {
	id: projectId,
	name: 'Graph project',
	description: 'A graph-only mocked project',
	default_max_depth: 2,
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z'
};

const dependencies = {
	postgresql: { state: 'available', lag: null, backlog: null, oldest_age_seconds: null },
	worker: { state: 'available', lag: 0, backlog: 0, oldest_age_seconds: null }
};

async function mockGraphWorkspace(
	page: Page
): Promise<{ graphFields: string[]; protocolRequested: boolean }> {
	const graphFields: string[] = [];
	let protocolRequested = false;
	await page.route(`${api}/health/dependencies`, (route) =>
		route.fulfill({ json: dependencies })
	);
	await page.route(/\/api\/projects(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [project], next_cursor: null } })
	);
	await page.route(`${api}/projects/${projectId}`, (route) => route.fulfill({ json: project }));
	await page.route(/\/api\/ingestions(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [], next_cursor: null } })
	);
	await page.route(/\/api\/projects\/project-1\/reports(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [], next_cursor: null } })
	);
	await page.route(
		new RegExp(`/api/projects/${projectId}/reports/00000000-0000-4000-8000-000000000001$`),
		(route) =>
			route.fulfill({
				json: {
					report_id: '00000000-0000-4000-8000-000000000001',
					title: 'Included report',
					doi: null,
					total_citations: 10,
					references_count: 2,
					issued_year: 2020,
					type: 'article',
					metrics_as_of: '2026-01-01T00:00:00Z',
					metrics_stale: false,
					internal_citations: 1,
					outbound_internal_references: 1,
					rank_score: 0.8
				}
			})
	);
	await page.route(`${api}/projects/${projectId}/protocol`, (route) => {
		protocolRequested = true;
		return route.fulfill({ status: 404, json: { message: 'no protocol' } });
	});
	await page.route(`${api}/projects/${projectId}/projection`, (route) =>
		route.fulfill({
			json: {
				project_id: projectId,
				state: 'ready',
				watermark: 3,
				revision: 3,
				lag: 0,
				last_success_at: '2026-01-01T00:00:00Z'
			}
		})
	);
	await page.route(new RegExp(`/api/projects/${projectId}/graph(?:\\?.*)?$`), async (route) => {
		const fields = new URL(route.request().url()).searchParams.get('fields') ?? '';
		graphFields.push(fields);
		const selected = new Set(fields.split(',').filter(Boolean));
		const overlay = (field: string, value: unknown) =>
			selected.has(field) ? value : undefined;
		await route.fulfill({
			json: {
				nodes: [
					{
						report_id: '00000000-0000-4000-8000-000000000001',
						title: 'Included report',
						metrics: overlay('metrics', {
							total_citations: 10,
							references_count: 2,
							internal_citations: 1,
							outbound_internal_references: 1,
							rank_score: 0.8,
							metrics_as_of: '2026-01-01T00:00:00Z',
							metrics_stale: false
						}),
						screening: overlay('screening', {
							title_abstract_status: 'include',
							full_text_status: 'include',
							final_status: 'include'
						}),
						study: overlay('study', { study_id: null, title: null }),
						appraisal: overlay('appraisal', {
							assessment_count: 0,
							completed_count: 0,
							latest_completed_at: null
						}),
						provenance: overlay('provenance', { sources: [], source_record_count: 0 })
					},
					{
						report_id: '00000000-0000-4000-8000-000000000002',
						title: 'Excluded report',
						metrics: overlay('metrics', {
							total_citations: 2,
							references_count: 1,
							internal_citations: 0,
							outbound_internal_references: 0,
							rank_score: 0.2,
							metrics_as_of: null,
							metrics_stale: false
						}),
						screening: overlay('screening', {
							title_abstract_status: 'exclude',
							full_text_status: 'not_required',
							final_status: 'exclude'
						})
					}
				],
				edges: [
					{
						source: '00000000-0000-4000-8000-000000000001',
						target: '00000000-0000-4000-8000-000000000002'
					}
				],
				projection: { revision: 3, lag: 0, last_success_at: '2026-01-01T00:00:00Z' },
				truncated: false
			}
		});
	});
	return {
		graphFields,
		get protocolRequested() {
			return protocolRequested;
		}
	};
}

test('graph overlay controls request selected fields and preserve graph-only neutral state', async ({
	page
}) => {
	const mock = await mockGraphWorkspace(page);
	await page.goto(
		`/projects/${projectId}/graph?graphFields=metrics,screening&graphColorBy=screening&report=00000000-0000-4000-8000-000000000001`
	);

	await expect(page.getByRole('heading', { name: 'Graph' })).toBeVisible();
	await expect(page.getByTestId('graph-overlay-legend')).toContainText('include');
	await expect(page.getByTestId('graph-overlay-legend')).toContainText('exclude');
	await expect(page.getByText('Screening: include')).toBeVisible();
	await expect(page.getByLabel('Color graph by')).toHaveValue('screening');
	await expect(page.getByLabel('Load study overlay')).not.toBeChecked();

	await page.getByLabel('Load study overlay').check();
	await expect(page).toHaveURL(/graphFields=metrics%2Cscreening%2Cstudy/);
	await expect.poll(() => mock.graphFields.at(-1) ?? '').toContain('study');
	await page.getByLabel('Color graph by').selectOption('study');
	await expect(page).toHaveURL(/graphColorBy=study/);
	await expect(page.getByTestId('graph-overlay-legend')).toContainText('grouped');
	await page.getByLabel('Color graph by').selectOption('metrics');
	await expect(page).not.toHaveURL(/graphColorBy=/);
	await expect(page.getByLabel('Color graph by')).toHaveValue('metrics');
	await expect(page.getByTestId('graph-overlay-legend')).toContainText('internally cited');
	await expect(page.getByTestId('graph-overlay-legend')).toContainText('no internal citations');
	await expect.poll(() => mock.protocolRequested).toBe(false);
});
