import { expect, test } from '@playwright/test';

test('screens the next report after an optimistic-safe decision', async ({ page }) => {
	let screened = false;
	const protocol = {
		id: 'protocol-1',
		version: 1,
		name: 'Default evidence screening protocol',
		status: 'published',
		criteria: [
			{
				id: 'population',
				label: 'Population',
				description: 'Matches the review population.'
			}
		],
		published_at: '2026-01-01T00:00:00Z'
	};
	const reports = [
		{
			report_id: 'report-1',
			title: 'Durable citation graphs',
			abstract_text: 'A first abstract.',
			doi: '10.5555/one',
			publication_year: 2024,
			title_abstract_status: 'unscreened',
			full_text_status: 'not_required',
			final_status: 'unscreened',
			revision: 0
		},
		{
			report_id: 'report-2',
			title: 'Rebuildable graph projections',
			abstract_text: 'A second abstract.',
			doi: '10.5555/two',
			publication_year: 2025,
			title_abstract_status: 'unscreened',
			full_text_status: 'not_required',
			final_status: 'unscreened',
			revision: 0
		}
	];

	await page.route('http://localhost:4173/api/projects/project-1/protocol', async (route) => {
		await route.fulfill({ json: protocol });
	});
	await page.route(
		'http://localhost:4173/api/projects/project-1/screening/title-abstract?status=unscreened&limit=100',
		async (route) => {
			await route.fulfill({
				json: {
					items: screened ? reports.slice(1) : reports,
					status: 'unscreened',
					total: screened ? 1 : 2
				}
			});
		}
	);
	await page.route(
		'http://localhost:4173/api/projects/project-1/reports/report-1/screening',
		async (route) => {
			expect(route.request().method()).toBe('POST');
			expect(route.request().postDataJSON()).toMatchObject({
				stage: 'title_abstract',
				decision: 'include',
				protocol_version_id: 'protocol-1',
				expected_revision: 0
			});
			screened = true;
			await route.fulfill({
				json: {
					project_id: 'project-1',
					report_id: 'report-1',
					title_abstract_status: 'include',
					full_text_status: 'not_required',
					final_status: 'pending_full_text',
					revision: 1,
					last_event_id: 'event-1',
					updated_at: '2026-01-01T00:00:00Z'
				}
			});
		}
	);

	await page.goto('/projects/project-1/screening/title-abstract');
	await expect(page.getByRole('heading', { name: 'Durable citation graphs' })).toBeVisible();
	await page.getByRole('button', { name: /Include/ }).click();
	await expect(
		page.getByRole('heading', { name: 'Rebuildable graph projections' })
	).toBeVisible();
	await expect(page.getByText('1 remaining')).toBeVisible();
});
