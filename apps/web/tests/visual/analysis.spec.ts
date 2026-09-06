import { captureDarkViewport, expect, settleVisualPage, test } from './fixtures';
import type { Page } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';

const projectId = 'visual-project';
const reportId = '00000000-0000-4000-8000-000000000001';
const api = `**/api/projects/${projectId}`;

async function assertNoHorizontalOverflow(page: Page) {
	const dimensions = await page.evaluate(() => ({
		bodyScrollWidth: document.body.scrollWidth,
		documentScrollWidth: document.documentElement.scrollWidth,
		viewportWidth: window.innerWidth
	}));
	expect(dimensions.bodyScrollWidth).toBeLessThanOrEqual(dimensions.viewportWidth);
	expect(dimensions.documentScrollWidth).toBeLessThanOrEqual(dimensions.viewportWidth);
}

async function runScopedSeriousCriticalAxe(page: Page, selector: string) {
	const results = await new AxeBuilder({ page })
		.include(selector)
		.withTags(['wcag2a', 'wcag2aa'])
		.analyze();
	return results.violations.filter(
		(violation) => violation.impact === 'serious' || violation.impact === 'critical'
	);
}

async function installPrismaFixture(page: Page) {
	await page.route(`${api}/prisma`, (route) =>
		route.fulfill({
			json: {
				project_id: projectId,
				as_of: '2026-01-15T12:00:00Z',
				identified_records: 12,
				linked_records: 10,
				duplicates_removed: 2,
				unresolved_records: 1,
				pending_dedupe_proposals: 0,
				source_canonical_reports: 8,
				manually_created_reports: 2,
				screened_records: 10,
				title_abstract_excluded: 2,
				title_abstract_pending: 0,
				reports_sought: 8,
				reports_not_retrieved: 1,
				full_text_assessed: 7,
				full_text_pending: 0,
				full_text_included: 5,
				full_text_excluded: 2,
				included_reports_not_grouped: 1,
				included_studies: 3,
				screening_high_watermark: 4,
				full_text_exclusions: [
					{ id: 'reason-1', code: 'wrong-design', label: 'Wrong design', count: 2 }
				]
			}
		})
	);
	await page.route(`${api}/exports/**`, (route) => {
		const kind = route.request().url().split('/').pop() ?? 'artifact';
		const body =
			kind === 'prisma.svg'
				? '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 360"><text x="20" y="40">Fixture PRISMA</text></svg>'
				: `${kind} fixture`;
		return route.fulfill({
			headers: {
				'content-type': kind === 'prisma.svg' ? 'image/svg+xml' : 'text/plain',
				'content-disposition': `attachment; filename="deepref-${projectId}-${kind}"`
			},
			body
		});
	});
}

async function installGraphFixture(page: Page) {
	await page.route(`${api}/graph**`, (route) =>
		route.fulfill({
			json: {
				nodes: [
					{
						report_id: reportId,
						title: 'Included fixture report',
						metrics: { internal_citations: 3, rank_score: 0.9 },
						screening: { final_status: 'include' }
					},
					{
						report_id: '00000000-0000-4000-8000-000000000002',
						title: 'Excluded fixture report',
						metrics: { internal_citations: 0, rank_score: 0.2 },
						screening: { final_status: 'exclude' }
					}
				],
				edges: [{ source: reportId, target: '00000000-0000-4000-8000-000000000002' }],
				projection: { revision: 42, lag: 0, last_success_at: '2026-01-15T12:00:00Z' },
				truncated: false
			}
		})
	);
}

async function installRecommendationsFixture(page: Page) {
	await page.route(`${api}/recommendations`, (route) =>
		route.fulfill({
			json: {
				foundational: [
					{
						report_id: reportId,
						title: 'Foundational fixture report',
						doi: '10.5555/foundational-fixture',
						internal_citations: 3,
						total_citations: 18
					}
				],
				core_to_project: [],
				underexplored: [],
				projection: { revision: 42, lag: 0, last_success_at: '2026-01-15T12:00:00Z' }
			}
		})
	);
}

test.describe('analysis workflow smoke', () => {
	test('PRISMA export presentation remains readable on every review viewport', async ({
		page
	}) => {
		await installPrismaFixture(page);
		await page.goto(`/projects/${projectId}/prisma`);
		await settleVisualPage(page);
		await expect(page.getByRole('heading', { name: 'PRISMA flow', exact: true })).toBeVisible();
		await expect(page.getByRole('img', { name: /PRISMA flow diagram/ })).toBeVisible();
		await expect(page.getByRole('button', { name: 'PRISMA PNG' })).toBeVisible();
		await assertNoHorizontalOverflow(page);
		expect(await runScopedSeriousCriticalAxe(page, '[data-testid="prisma-page"]')).toEqual([]);
		await captureDarkViewport(page, 'analyze-prisma.png');
	});

	test('Graph overlays remain legible and keyboard-addressable on every review viewport', async ({
		page
	}) => {
		await installGraphFixture(page);
		await page.goto(
			`/projects/${projectId}/graph?graphFields=metrics,screening&graphColorBy=screening&report=${reportId}`
		);
		await settleVisualPage(page);
		await expect(page.getByRole('heading', { name: 'Graph', exact: true })).toBeVisible();
		await expect(page.getByTestId('graph-overlay-legend')).toContainText('include');
		await expect(page.getByText('Screening: include')).toBeVisible();
		await expect(page.getByLabel('Color graph by')).toHaveValue('screening');
		await assertNoHorizontalOverflow(page);
		await captureDarkViewport(page, 'analyze-graph.png');
		expect(await runScopedSeriousCriticalAxe(page, '[data-testid="graph-page"]')).toEqual([]);
	});

	test('Recommendations retain category hierarchy and selected article affordance', async ({
		page
	}) => {
		await installRecommendationsFixture(page);
		await page.goto(`/projects/${projectId}/recommendations`);
		await settleVisualPage(page);
		await expect(
			page.getByRole('heading', { name: 'Recommendations', exact: true })
		).toBeVisible();
		await expect(
			page.getByRole('heading', { name: 'Foundational', exact: true })
		).toBeVisible();
		await expect(
			page.getByRole('button', { name: 'Open Foundational fixture report' })
		).toBeVisible();
		await assertNoHorizontalOverflow(page);
		expect(
			await runScopedSeriousCriticalAxe(page, '[data-testid="recommendations-page"]')
		).toEqual([]);
		await captureDarkViewport(page, 'analyze-recommendations.png');
	});
});
