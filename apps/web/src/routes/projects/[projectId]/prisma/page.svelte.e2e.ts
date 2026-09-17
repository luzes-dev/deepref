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

const canonicalSvg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 360">
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
	await page.getByText('All review counts', { exact: true }).click();
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
