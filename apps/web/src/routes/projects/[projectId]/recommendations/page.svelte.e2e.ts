import { expect, test, type Page } from '@playwright/test';

const projectId = 'project-1';
const reportId = '00000000-0000-4000-8000-000000000001';
const api = 'http://localhost:4173/api';
const project = {
	id: projectId,
	name: 'Recommendations project',
	description: 'A recommendation fixture',
	default_max_depth: 2,
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z'
};

const dependencies = {
	postgresql: { state: 'available', lag: null, backlog: null, oldest_age_seconds: null },
	worker: { state: 'available', lag: 0, backlog: 0, oldest_age_seconds: null }
};

async function mockRecommendationsWorkspace(page: Page): Promise<void> {
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
	await page.route(`${api}/projects/${projectId}/recommendations`, (route) =>
		route.fulfill({
			json: {
				foundational: [
					{
						report_id: reportId,
						title: 'Foundational evidence report',
						doi: '10.5555/foundational',
						internal_citations: 4,
						total_citations: 12
					}
				],
				core_to_project: [],
				underexplored: [],
				projection: { revision: 3, lag: 0, last_success_at: '2026-01-01T00:00:00Z' }
			}
		})
	);
}

test('recommendations preserve projection metadata, category hierarchy, and article selection', async ({
	page
}) => {
	await mockRecommendationsWorkspace(page);
	await page.goto(`/projects/${projectId}/recommendations`);

	await expect(page.getByRole('heading', { name: 'Recommendations', exact: true })).toBeVisible();
	await expect(page.getByText('Projection revision 3', { exact: true })).toBeVisible();
	await expect(page.getByRole('heading', { name: 'Foundational', exact: true })).toBeVisible();
	await expect(page.getByText('Foundational evidence report', { exact: true })).toBeVisible();
	await expect(page.getByText('Internal 4', { exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Open Foundational evidence report' }).click();
	await expect(page).toHaveURL(`/projects/${projectId}/articles?report=${reportId}`);
});
