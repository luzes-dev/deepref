import { captureDarkViewport, expect, settleVisualPage, test } from './fixtures';
import type { Page } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';

const projectId = 'visual-project';
const reportId = '00000000-0000-4000-8000-000000000001';
const api = `**/api/projects/${projectId}`;
const prismaFixtureSvg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 360">
<rect width="640" height="360" fill="#fff"/>
<defs><marker id="arrow" markerWidth="8" markerHeight="8" refX="7" refY="4" orient="auto"><path d="M0,0 L8,4 L0,8 z" fill="#0a6b58"/></marker></defs>
<g font-family="Arial, sans-serif" fill="#1d302c">
<text x="24" y="26" font-size="16" font-weight="700">PRISMA 2020 flow · review fixture</text>
<text x="24" y="48" font-size="11" font-weight="700" fill="#587069">IDENTIFICATION</text>
<text x="366" y="48" font-size="11" font-weight="700" fill="#587069">SCREENING AND INCLUSION</text>
<g stroke="#0a6b58" stroke-width="1.5">
<rect x="24" y="60" width="250" height="48" rx="6" fill="#e7f3ef"/><rect x="24" y="126" width="250" height="48" rx="6" fill="#f4f7f5"/><rect x="24" y="192" width="250" height="48" rx="6" fill="#e7f3ef"/><rect x="24" y="258" width="250" height="48" rx="6" fill="#f4f7f5"/>
<rect x="366" y="60" width="250" height="48" rx="6" fill="#e7f3ef"/><rect x="366" y="126" width="250" height="48" rx="6" fill="#f4f7f5"/><rect x="366" y="192" width="250" height="48" rx="6" fill="#e7f3ef"/><rect x="366" y="258" width="250" height="48" rx="6" fill="#dcefe9"/>
</g>
<g font-size="13" font-weight="700"><text x="38" y="82">Records identified</text><text x="38" y="99" font-size="11" font-weight="400">n = 12</text><text x="38" y="148">After duplicates removed</text><text x="38" y="165" font-size="11" font-weight="400">n = 10 · duplicates n = 2</text><text x="38" y="214">Records screened</text><text x="38" y="231" font-size="11" font-weight="400">n = 10</text><text x="38" y="280">Title/abstract excluded</text><text x="38" y="297" font-size="11" font-weight="400">n = 2</text><text x="380" y="82">Reports sought</text><text x="380" y="99" font-size="11" font-weight="400">n = 8</text><text x="380" y="148">Reports not retrieved</text><text x="380" y="165" font-size="11" font-weight="400">n = 1</text><text x="380" y="214">Full texts assessed</text><text x="380" y="231" font-size="11" font-weight="400">n = 7</text><text x="380" y="280">Included studies</text><text x="380" y="297" font-size="11" font-weight="400">n = 3</text></g>
<g fill="none" stroke="#0a6b58" stroke-width="2" marker-end="url(#arrow)"><path d="M149 108 V126"/><path d="M149 174 V192"/><path d="M149 240 V258"/><path d="M274 84 H366"/><path d="M491 108 V126"/><path d="M491 174 V192"/><path d="M491 240 V258"/></g>
</g></svg>`;

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
		const body = kind === 'prisma.svg' ? prismaFixtureSvg : `${kind} fixture`;
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
		const diagram = page.getByRole('img', { name: /PRISMA flow diagram/ });
		await expect(diagram).toBeVisible();
		await expect
			.poll(async () =>
				diagram.evaluate(
					(element) =>
						element instanceof HTMLImageElement &&
						element.complete &&
						element.naturalWidth > 0
				)
			)
			.toBe(true);
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
