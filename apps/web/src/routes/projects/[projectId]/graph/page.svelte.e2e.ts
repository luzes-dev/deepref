import { expect, test } from '@playwright/test';

test('renders graph canvas on initial load', async ({ page }) => {
	const pageErrors: Error[] = [];
	page.on('pageerror', (error) => pageErrors.push(error));
	await page.route('http://localhost:8080/projects/test-project/graph', async (route) => {
		await route.fulfill({
			json: {
				nodes: [
					{
						doi: '10.1/source',
						doi_key: 'MTAuMS9zb3VyY2U',
						title: 'Source',
						issued_year: 2024,
						type: 'article',
						total_citations: 10,
						internal_citations: 2,
						outbound_internal_references: 1,
						rank_score: 0.8
					},
					{
						doi: '10.1/target',
						doi_key: 'MTAuMS90YXJnZXQ',
						title: 'Target',
						issued_year: 2023,
						type: 'article',
						total_citations: 8,
						internal_citations: 1,
						outbound_internal_references: 1,
						rank_score: 0.5
					},
					{
						doi: '10.1/third',
						doi_key: 'MTAuMS90aGlyZA',
						title: 'Third',
						issued_year: 2022,
						type: 'article',
						total_citations: 3,
						internal_citations: 0,
						outbound_internal_references: 0,
						rank_score: 0.2
					}
				],
				edges: [
					{ source: '10.1/source', target: '10.1/target' },
					{ source: '10.1/target', target: '10.1/third' },
					{ source: '10.1/third', target: '10.1/third' }
				]
			}
		});
	});

	await page.goto('/projects/test-project/graph');

	await expect(page.getByText('3 nodes and 3 edges')).toBeVisible();
	await expect(page.locator('canvas').first()).toBeVisible();
	await expect(page.locator('canvas')).toHaveCount(7);
	await page.getByRole('button', { name: 'Reset layout' }).click();
	await expect(page.locator('canvas')).toHaveCount(7);
	expect(pageErrors).toEqual([]);
});
