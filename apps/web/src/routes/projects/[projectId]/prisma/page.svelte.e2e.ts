import { expect, test, type Page } from '@playwright/test';

const projectId = 'project-1';
const api = 'http://localhost:4173/api';
const project = {
	id: projectId,
	name: 'PRISMA project',
	description: 'A canonical PRISMA fixture',
	default_max_depth: 2,
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z'
};

const dependencies = {
	postgresql: { state: 'available', lag: null, backlog: null, oldest_age_seconds: null },
	worker: { state: 'available', lag: 0, backlog: 0, oldest_age_seconds: null }
};

const projection = {
	project_id: projectId,
	as_of: '2026-01-01T00:00:00Z',
	identified_records: 8,
	linked_records: 6,
	duplicates_removed: 2,
	unresolved_records: 2,
	pending_dedupe_proposals: 1,
	source_canonical_reports: 4,
	manually_created_reports: 2,
	screened_records: 6,
	title_abstract_excluded: 1,
	title_abstract_pending: 1,
	reports_sought: 4,
	reports_not_retrieved: 1,
	full_text_assessed: 3,
	full_text_pending: 1,
	full_text_included: 1,
	full_text_excluded: 1,
	included_reports_not_grouped: 1,
	included_studies: 0,
	screening_high_watermark: 3,
	full_text_exclusions: [
		{ id: 'reason-1', code: 'wrong-design', label: 'Wrong design', count: 1 }
	]
};

const canonicalSvg =
	'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 360"><text x="20" y="40">Canonical PRISMA</text></svg>';

async function mockPrismaWorkspace(page: Page): Promise<void> {
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
	await page.route(`${api}/projects/${projectId}/prisma`, (route) =>
		route.fulfill({ json: projection })
	);
	await page.route(new RegExp(`/api/projects/${projectId}/exports/[^/]+$`), async (route) => {
		const kind = route.request().url().split('/').pop();
		if (kind === 'audit.csv') {
			await route.fulfill({
				status: 500,
				headers: { 'content-type': 'application/json' },
				json: { code: 'EXPORT_FAILED', message: 'audit export failed' }
			});
			return;
		}
		const body = kind === 'prisma.svg' ? canonicalSvg : `${kind} fixture`;
		const contentType =
			kind === 'prisma.svg' ? 'image/svg+xml; charset=utf-8' : 'text/csv; charset=utf-8';
		await route.fulfill({
			status: 200,
			headers: {
				'content-type': contentType,
				'content-disposition': `attachment; filename="deepref-${projectId}-${kind}"`
			},
			body
		});
	});
}

test('PRISMA page renders canonical reconciliation and deterministic exports', async ({ page }) => {
	await mockPrismaWorkspace(page);
	await page.goto(`/projects/${projectId}/prisma`);

	await expect(page.getByRole('heading', { name: 'PRISMA flow' })).toBeVisible();
	for (const [label, value] of [
		['Screened records', '6'],
		['Title/abstract excluded', '1'],
		['Title/abstract pending', '1'],
		['Reports sought', '4'],
		['Reports not retrieved', '1'],
		['Full texts assessed', '3'],
		['Full-text pending', '1'],
		['Full-text included', '1'],
		['Full-text excluded', '1'],
		['Grouped reports', '0']
	] as const) {
		await expect(page.getByText(label, { exact: true }).locator('..')).toContainText(value);
	}
	await expect(page.getByText('Source-canonical reports', { exact: true })).toBeVisible();
	await expect(page.getByText('Manually created reports', { exact: true })).toBeVisible();
	await expect(page.getByRole('listitem')).toContainText('Wrong design (wrong-design)');
	await expect(page.getByRole('img', { name: /PRISMA flow diagram/ })).toBeVisible();

	for (const label of [
		'Reports CSV',
		'Reports JSON',
		'Reports RIS',
		'Reports BibTeX',
		'PRISMA JSON',
		'PRISMA SVG',
		'Audit CSV',
		'Protocol snapshot',
		'PRISMA PNG'
	]) {
		await expect(page.getByRole('button', { name: label, exact: true })).toBeVisible();
	}

	const downloadPromise = page.waitForEvent('download');
	await page.getByRole('button', { name: 'Reports CSV', exact: true }).click();
	const download = await downloadPromise;
	expect(download.suggestedFilename()).toBe(`deepref-${projectId}-reports.csv`);

	await page.getByRole('button', { name: 'Audit CSV', exact: true }).click();
	await expect(page.getByRole('alert')).toContainText('audit export failed');
});
