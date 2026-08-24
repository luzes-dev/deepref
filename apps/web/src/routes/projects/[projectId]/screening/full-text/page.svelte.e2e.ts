import { expect, test, type Page } from '@playwright/test';

const api = 'http://localhost:4173/api';
const projectId = 'project-1';
const reportId = 'report-1';
const documentId = 'document-1';

function pdfFixture(): Buffer {
	const content = 'BT /F1 16 Tf 72 720 Td (Evidence text) Tj ET\n';
	const objects = [
		'1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n',
		'2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n',
		'3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>\nendobj\n',
		`4 0 obj\n<< /Length ${Buffer.byteLength(content)} >>\nstream\n${content}endstream\nendobj\n`,
		'5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n'
	];
	let source = '%PDF-1.4\n';
	const offsets = [0];
	for (const object of objects) {
		offsets.push(Buffer.byteLength(source));
		source += object;
	}
	const xref = Buffer.byteLength(source);
	source += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
	source += offsets
		.slice(1)
		.map((offset) => `${String(offset).padStart(10, '0')} 00000 n \n`)
		.join('');
	source += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
	return Buffer.from(source);
}

const project = {
	id: projectId,
	name: 'Screening project',
	description: 'A mocked project',
	default_max_depth: 2,
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z'
};

const protocol = {
	id: 'protocol-1',
	project_id: projectId,
	version: 2,
	revision: 1,
	name: 'Full-text protocol',
	objective: 'Identify relevant evidence',
	question: 'Does this report answer the review question?',
	framework_kind: 'pico',
	framework_fields: {},
	status: 'published',
	criteria: [
		{
			id: 'full-text',
			label: 'Full text',
			description: 'Meets the protocol.',
			kind: 'inclusion',
			dimension: 'outcome',
			stage: 'full_text',
			ordinal: 1
		}
	],
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z',
	published_at: '2026-01-01T00:00:00Z',
	amendment_of: null
};

const report = {
	report_id: reportId,
	title: 'A report with an available full text',
	abstract_text: 'A deterministic abstract fixture.',
	doi: '10.5555/example',
	publication_year: 2024,
	title_abstract_status: 'include',
	full_text_status: 'unscreened',
	final_status: 'pending_full_text',
	revision: 4
};

const availableDocument = {
	id: documentId,
	report_id: reportId,
	mime_type: 'application/pdf',
	byte_size: 128,
	content_hash: 'sha256-fixture',
	original_filename: 'evidence.pdf',
	source: 'upload',
	status: 'available',
	ocr_required: false,
	parser_version: 'fixture-parser-v1',
	parser_error: null,
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z'
};

const newerFailedDocument = {
	...availableDocument,
	id: 'document-failed-newer',
	status: 'failed',
	parser_version: null,
	parser_error: 'retrieval failed',
	created_at: '2026-01-02T00:00:00Z',
	updated_at: '2026-01-02T00:00:00Z'
};

function paginatedQueueItem(index: number) {
	return {
		report_id: `report-${index}`,
		title: `Cursor report ${index}`,
		abstract_text: `Abstract ${index}`,
		doi: null,
		publication_year: 2025,
		full_text_status: 'unscreened',
		revision: 1,
		document: null
	};
}

async function installMocks(page: Page, options: { paginated?: boolean } = {}) {
	let attached = false;
	let currentStatus = 'unscreened';
	let revision = report.revision;
	let conflictNext = false;
	const decisionBodies: unknown[] = [];

	await page.route(`${api}/health/dependencies`, (route) =>
		route.fulfill({
			json: {
				postgresql: {
					state: 'available',
					lag: null,
					backlog: null,
					oldest_age_seconds: null
				},
				worker: { state: 'available', lag: 0, backlog: 0, oldest_age_seconds: null }
			}
		})
	);
	await page.route(/\/api\/projects(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [project], next_cursor: null } })
	);
	await page.route(`${api}/projects/${projectId}`, (route) => route.fulfill({ json: project }));
	await page.route(`${api}/projects/${projectId}/protocol`, (route) =>
		route.fulfill({ json: protocol })
	);
	await page.route(/\/api\/projects\/project-1\/reports(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [], next_cursor: null } })
	);
	await page.route(/\/api\/ingestions(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [], next_cursor: null } })
	);
	await page.route(
		(url) => url.pathname === `/api/projects/${projectId}/screening/full-text`,
		(route) => {
			if (options.paginated) {
				const cursor = new URL(route.request().url()).searchParams.get('cursor');
				return route.fulfill({
					json: cursor
						? { items: [paginatedQueueItem(101)], next_cursor: null }
						: {
								items: Array.from({ length: 100 }, (_, index) =>
									paginatedQueueItem(index + 1)
								),
								next_cursor: 'after-100'
							}
				});
			}
			return route.fulfill({
				json: {
					items: [
						{
							report_id: reportId,
							title: report.title,
							abstract_text: report.abstract_text,
							doi: report.doi,
							publication_year: report.publication_year,
							full_text_status: currentStatus,
							revision,
							document: attached ? availableDocument : null
						}
					],
					next_cursor: null
				}
			});
		}
	);
	await page.route(
		new RegExp(`/api/projects/${projectId}/screening/full-text/missing(?:\\?.*)?$`),
		(route) =>
			route.fulfill({
				json: attached
					? []
					: [
							{
								report_id: reportId,
								title: report.title,
								abstract_text: report.abstract_text,
								status: 'missing'
							}
						]
			})
	);
	await page.route(`${api}/projects/${projectId}/screening/full-text/reasons`, (route) =>
		route.fulfill({
			json: [
				{
					id: 'reason-1',
					code: 'wrong_comparator_outcome',
					label: 'Wrong comparator/outcome',
					stage: 'full_text'
				}
			]
		})
	);
	await page.route(
		new RegExp(`/api/projects/${projectId}/reports/${reportId}/documents(?:\\?.*)?$`),
		async (route) => {
			if (route.request().method() === 'POST') {
				attached = true;
				return route.fulfill({ status: 201, json: { data: availableDocument } });
			}
			return route.fulfill({
				json: attached ? [newerFailedDocument, availableDocument] : []
			});
		}
	);
	await page.route(
		new RegExp(`/api/projects/${projectId}/reports/report-(?:100|101)/documents(?:\\?.*)?$`),
		(route) => route.fulfill({ json: [] })
	);
	await page.route(
		`${api}/projects/${projectId}/reports/${reportId}/documents/${documentId}`,
		(route) => route.fulfill({ json: availableDocument })
	);
	await page.route(
		new RegExp(
			`/api/projects/${projectId}/reports/${reportId}/documents/${documentId}/blocks(?:\\?.*)?$`
		),
		(route) =>
			route.fulfill({
				json: [
					{
						id: 'block-1',
						document_id: documentId,
						parser_version: 'fixture-parser-v1',
						page_number: 1,
						kind: 'paragraph',
						section_path: [],
						ordinal: 1,
						text: 'Evidence text',
						content_hash: 'block-hash',
						bbox: { x: 0.1, y: 0.2, width: 0.4, height: 0.1 }
					}
				]
			})
	);
	await page.route(
		`${api}/projects/${projectId}/reports/${reportId}/documents/${documentId}/pages`,
		(route) =>
			route.fulfill({
				json: [
					{
						document_id: documentId,
						parser_version: 'fixture-parser-v1',
						page_number: 1,
						width: 612,
						height: 792,
						ocr_required: false
					}
				]
			})
	);
	await page.route(
		`${api}/projects/${projectId}/reports/${reportId}/documents/${documentId}/content`,
		(route) =>
			route.fulfill({
				status: 200,
				contentType: 'application/pdf',
				body: pdfFixture()
			})
	);
	await page.route(
		new RegExp(`/api/projects/${projectId}/reports/${reportId}/screening/history$`),
		(route) =>
			route.fulfill({ json: { project_id: projectId, report_id: reportId, items: [] } })
	);
	await page.route(
		new RegExp(`/api/projects/${projectId}/reports/report-(?:100|101)/screening/history$`),
		(route) =>
			route.fulfill({
				json: { project_id: projectId, report_id: 'cursor-report', items: [] }
			})
	);
	await page.route(
		new RegExp(`/api/projects/${projectId}/reports/${reportId}/screening$`),
		async (route) => {
			if (route.request().method() !== 'POST') return route.continue();
			const decisionBody = route.request().postDataJSON() as {
				decision: string;
				exclusion_reason_id?: string;
			};
			decisionBodies.push(decisionBody);
			if (conflictNext) {
				conflictNext = false;
				currentStatus = 'maybe';
				revision = 8;
				return route.fulfill({
					status: 409,
					json: {
						code: 'screening_revision_conflict',
						message: 'revision conflict',
						details: {
							currentState: {
								report_id: reportId,
								full_text_status: currentStatus,
								full_text_exclusion_reason_id: null,
								revision
							}
						}
					}
				});
			}
			currentStatus = decisionBody.decision;
			revision += 1;
			return route.fulfill({
				json: {
					project_id: projectId,
					report_id: reportId,
					title_abstract_status: 'include',
					full_text_status: currentStatus,
					full_text_exclusion_reason_id: decisionBody.exclusion_reason_id ?? null,
					final_status: currentStatus,
					revision,
					last_event_id: null,
					updated_at: '2026-01-01T00:00:00Z'
				}
			});
		}
	);

	return {
		decisionBodies,
		conflictOnce: () => {
			conflictNext = true;
		}
	};
}

test('keeps the included report selected through attachment and evidence navigation', async ({
	page
}) => {
	await page.setViewportSize({ width: 480, height: 900 });
	const mock = await installMocks(page);
	await page.goto(`/projects/${projectId}/screening/full-text?filter=missing&report=${reportId}`);
	await expect(page.getByText(report.title, { exact: true })).toBeVisible();
	await expect(page.getByRole('button', { name: 'Include', exact: true })).toBeDisabled();

	await page.locator('input[type="file"]').setInputFiles({
		name: 'evidence.pdf',
		mimeType: 'application/pdf',
		buffer: pdfFixture()
	});
	await expect(page.getByRole('button', { name: /Evidence text/ })).toBeVisible();
	await expect(page.getByText('Attached · left missing queue')).toBeVisible();
	await expect(page.getByRole('button', { name: 'Include', exact: true })).toBeEnabled();
	await expect(page.getByRole('button', { name: 'Exclude', exact: true })).toBeDisabled();

	await page.getByRole('button', { name: /Evidence text/ }).click();
	await expect(page).toHaveURL(/report=report-1.*page=1.*block=block-1/);
	await expect(page.getByRole('button', { name: 'Evidence block on page 1' })).toHaveAttribute(
		'aria-pressed',
		'true'
	);
	const canvasBox = await page.getByLabel('PDF page 1').boundingBox();
	const overlayBox = await page
		.getByRole('button', { name: 'Evidence block on page 1' })
		.boundingBox();
	expect(canvasBox).not.toBeNull();
	expect(overlayBox).not.toBeNull();
	if (!canvasBox || !overlayBox) throw new Error('PDF geometry was not rendered');
	expect(canvasBox.width).toBeGreaterThan(700);
	expect(Math.abs(overlayBox.x - canvasBox.x - canvasBox.width * 0.1)).toBeLessThan(5);
	expect(Math.abs(overlayBox.y - canvasBox.y - canvasBox.height * 0.2)).toBeLessThan(5);
	expect(Math.abs(overlayBox.width - canvasBox.width * 0.4)).toBeLessThan(5);
	expect(overlayBox.x + overlayBox.width).toBeLessThanOrEqual(canvasBox.x + canvasBox.width + 1);
	await page.goBack();
	await expect(page).not.toHaveURL(/block=block-1/);
	await page.goForward();
	await expect(page).toHaveURL(/page=1.*block=block-1/);
	await page.getByRole('button', { name: 'Include', exact: true }).click();
	await expect(page.getByText('Full-text include recorded.')).toBeVisible();

	const body = mock.decisionBodies.at(-1);
	expect(body).toBeTruthy();
	expect(body).toMatchObject({ decision: 'include', expected_revision: 4 });
	expect(body).not.toHaveProperty('exclusion_reason_id');
	await page.reload();
	await expect(page).toHaveURL(/report=report-1.*page=1.*block=block-1/);
});

test('requires one reason for exclusion and reconciles a revision conflict', async ({ page }) => {
	const mock = await installMocks(page);
	await page.goto(`/projects/${projectId}/screening/full-text?report=${reportId}`);
	await page.locator('input[type="file"]').setInputFiles({
		name: 'evidence.pdf',
		mimeType: 'application/pdf',
		buffer: pdfFixture()
	});
	await expect(page.getByRole('button', { name: 'Exclude', exact: true })).toBeDisabled();
	await page.getByLabel('Primary full-text exclusion reason').selectOption('reason-1');
	await page.getByRole('button', { name: 'Exclude', exact: true }).click();
	await expect(page.getByText('Full-text exclude recorded.')).toBeVisible();
	expect(mock.decisionBodies.at(-1)).toMatchObject({
		decision: 'exclude',
		exclusion_reason_id: 'reason-1',
		expected_revision: 4
	});

	mock.conflictOnce();
	await page.getByRole('button', { name: 'Include', exact: true }).click();
	await expect(page.getByText(/authoritative state is loaded/)).toBeVisible();
	await page.getByRole('button', { name: 'Include', exact: true }).click();
	expect(mock.decisionBodies.at(-1)).toMatchObject({
		decision: 'include',
		expected_revision: 8
	});
	expect(mock.decisionBodies.at(-1)).not.toHaveProperty('exclusion_reason_id');
});

test('loads the next bounded cursor page and reaches report 101 through navigation', async ({
	page
}) => {
	await installMocks(page, { paginated: true });
	await page.goto(`/projects/${projectId}/screening/full-text?report=report-100`);
	await expect(page.getByText('Cursor report 100', { exact: true })).toBeVisible();
	await expect(page.getByText('100 of 100 loaded')).toBeVisible();
	await page.getByRole('button', { name: 'Load more reports' }).click();
	await expect(page.getByText('100 of 101 loaded')).toBeVisible();
	await page.getByRole('button', { name: 'Next' }).click();
	await expect(page).toHaveURL(/report=report-101/);
	await expect(page.getByText('Cursor report 101', { exact: true })).toBeVisible();
});
