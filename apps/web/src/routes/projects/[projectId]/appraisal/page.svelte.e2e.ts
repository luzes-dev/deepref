import { expect, test, type Page } from '@playwright/test';

const project = {
	id: 'project-1',
	name: 'Appraisal project',
	description: 'A mocked project',
	default_max_depth: 2,
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z'
};
const dependencies = {
	postgresql: { state: 'available', lag: null, backlog: null, oldest_age_seconds: null },
	worker: { state: 'available', lag: 0, backlog: 0, oldest_age_seconds: null }
};
const report = {
	report_id: 'report-1',
	title: 'Primary trial report',
	doi: null,
	issued_year: 2024,
	type: 'journal-article',
	rank_score: 0,
	total_citations: 0,
	internal_citations: 0,
	outbound_internal_references: 0,
	metrics_as_of: null,
	metrics_stale: false
};
const definitions = [
	{
		id: 'deepref-rct-generic',
		version: 1,
		name: 'DeepRef generic intervention appraisal',
		description: 'Intervention schema',
		applicability: { designs: ['rct'], note: null },
		domains: [
			{
				id: 'allocation',
				label: 'Allocation process',
				description: null,
				judgment: {
					options: [{ value: 'low_concern', label: 'Low concern' }],
					allow_custom: true,
					required: true
				},
				questions: [
					{
						id: 'allocation_description',
						label: 'Was the allocation process described?',
						help: null,
						answer_schema: { kind: 'enum', options: [{ value: 'yes', label: 'Yes' }] },
						required: true,
						requires_evidence: true
					}
				]
			},
			{
				id: 'outcome_reporting',
				label: 'Outcome reporting',
				description: null,
				judgment: {
					options: [{ value: 'low_concern', label: 'Low concern' }],
					allow_custom: true,
					required: true
				},
				questions: [
					{
						id: 'outcome_measure_prespecified',
						label: 'Was the outcome measure prespecified?',
						help: null,
						answer_schema: { kind: 'boolean' },
						required: true,
						requires_evidence: false
					}
				]
			}
		],
		overall_judgment: {
			options: [{ value: 'low_concern', label: 'Low concern' }],
			allow_custom: true,
			required: true
		}
	},
	{
		id: 'deepref-qualitative-generic',
		version: 1,
		name: 'DeepRef generic qualitative appraisal',
		description: 'Qualitative schema',
		applicability: { designs: ['qualitative'], note: null },
		domains: [
			{
				id: 'methodological_transparency',
				label: 'Methodological transparency',
				description: null,
				judgment: {
					options: [{ value: 'adequate', label: 'Adequate' }],
					allow_custom: true,
					required: true
				},
				questions: [
					{
						id: 'transparency_score',
						label: 'How clearly is the method described?',
						help: null,
						answer_schema: {
							kind: 'scale',
							min: 0,
							max: 2,
							labels: { '0': 'Not described', '2': 'Clearly described' }
						},
						required: true,
						requires_evidence: true
					},
					{
						id: 'reflexivity_note',
						label: 'Reviewer reflexivity note',
						help: null,
						answer_schema: { kind: 'text', max_length: 2000 },
						required: false,
						requires_evidence: false
					}
				]
			}
		],
		overall_judgment: {
			options: [{ value: 'adequate', label: 'Adequate' }],
			allow_custom: true,
			required: true
		}
	}
];

definitions.splice(1, 0, {
	...definitions[0],
	version: 2,
	name: 'DeepRef generic intervention appraisal revision',
	description: 'Intervention schema v2'
});

async function mockShell(page: Page): Promise<void> {
	await page.route('http://localhost:4173/api/health/dependencies', async (route) =>
		route.fulfill({ json: dependencies })
	);
	await page.route(/http:\/\/localhost:4173\/api\/projects(?:\?.*)?$/, async (route) =>
		route.fulfill({ json: { items: [project], next_cursor: null } })
	);
	await page.route('http://localhost:4173/api/projects/project-1', async (route) =>
		route.fulfill({ json: project })
	);
	await page.route(/http:\/\/localhost:4173\/api\/ingestions(?:\?.*)?$/, async (route) =>
		route.fulfill({ json: { items: [], next_cursor: null } })
	);
	await page.route(
		/http:\/\/localhost:4173\/api\/projects\/project-1\/reports(?:\?.*)?$/,
		async (route) => route.fulfill({ json: { items: [report], next_cursor: null } })
	);
	await page.route(
		'http://localhost:4173/api/projects/project-1/appraisal-definitions',
		async (route) => route.fulfill({ json: definitions })
	);
	await page.route(
		'http://localhost:4173/api/projects/project-1/reports/report-1/documents?limit=100',
		async (route) =>
			route.fulfill({
				json: [
					{
						id: 'document-1',
						report_id: 'report-1',
						source: 'upload',
						status: 'available',
						byte_size: 1,
						mime_type: 'application/pdf',
						ocr_required: false,
						created_at: '2026-01-01T00:00:00Z',
						updated_at: '2026-01-01T00:00:00Z',
						original_filename: 'trial.pdf',
						content_hash: 'a'.repeat(64),
						parser_version: 'parser-1',
						parser_error: null
					}
				]
			})
	);
	await page.route(
		'http://localhost:4173/api/projects/project-1/reports/report-1/documents/document-1/blocks?limit=100',
		async (route) =>
			route.fulfill({
				json: [
					{
						id: 'block-1',
						document_id: 'document-1',
						page_number: 1,
						ordinal: 0,
						kind: 'text',
						text: 'The allocation process was described.',
						section_path: [],
						content_hash: 'a'.repeat(64),
						parser_version: 'parser-1',
						page_ocr_required: false
					},
					{
						id: 'block-2',
						document_id: 'document-1',
						page_number: 1,
						ordinal: 1,
						kind: 'text',
						text: 'The allocation sequence was reported.',
						section_path: [],
						content_hash: 'b'.repeat(64),
						parser_version: 'parser-1',
						page_ocr_required: false
					}
				]
			})
	);
	await page.route(
		'http://localhost:4173/api/projects/project-1/reports/report-1/appraisals',
		async (route) => {
			if (route.request().method() === 'GET') await route.fulfill({ json: [] });
			else
				await route.fulfill({
					status: 201,
					json: {
						id: 'assessment-1',
						project_id: project.id,
						report_id: report.report_id,
						definition_id: 'deepref-rct-generic',
						definition_version: 1,
						responses: {},
						judgments: {},
						evidence: [],
						actor_kind: 'user',
						actor_id: 'tester',
						completed_at: '2026-01-01T00:00:00Z',
						created_at: '2026-01-01T00:00:00Z'
					}
				});
		}
	);
}

test('renders and submits both generic appraisal shapes with required evidence', async ({
	page
}) => {
	await mockShell(page);
	const payloads: Record<string, unknown>[] = [];
	await page.route(
		'http://localhost:4173/api/projects/project-1/reports/report-1/appraisals',
		async (route) => {
			if (route.request().method() === 'POST') {
				payloads.push(route.request().postDataJSON() as Record<string, unknown>);
			}
			await route.fallback();
		}
	);
	await page.goto(
		'/projects/project-1/appraisal?report=report-1&definition=deepref-rct-generic&definition_version=1'
	);
	await expect(page.getByRole('heading', { name: 'Appraisal', exact: true })).toBeVisible();
	await expect(
		page.getByRole('heading', {
			name: 'DeepRef generic intervention appraisal v1',
			exact: true
		})
	).toBeVisible();
	await expect(
		page.getByRole('button', { name: /DeepRef generic intervention appraisal v1/ })
	).toBeVisible();
	await expect(
		page.getByRole('button', { name: /DeepRef generic intervention appraisal revision/ })
	).toBeVisible();
	await page
		.getByRole('button', { name: /DeepRef generic intervention appraisal revision/ })
		.click();
	await expect(page).toHaveURL(/definition=deepref-rct-generic&definition_version=2/);
	await expect(
		page.getByRole('heading', {
			name: 'DeepRef generic intervention appraisal revision v2',
			exact: true
		})
	).toBeVisible();
	await page.getByRole('button', { name: /DeepRef generic intervention appraisal v1/ }).click();
	await expect(page).toHaveURL(/definition=deepref-rct-generic&definition_version=1/);
	await expect(
		page.getByRole('heading', {
			name: 'DeepRef generic intervention appraisal v1',
			exact: true
		})
	).toBeVisible();
	await page.locator('#allocation_description').selectOption('yes');
	await page.getByRole('button', { name: 'Add evidence block' }).first().click();
	await page.locator('#allocation_description-evidence-0').selectOption('block-1');
	await page.getByRole('button', { name: 'Add evidence block' }).first().click();
	await page.locator('#allocation_description-evidence-1').selectOption('block-2');
	await page.locator('#outcome_measure_prespecified').check();
	await page.locator('#allocation-judgment').selectOption('low_concern');
	await page.locator('#outcome_reporting-judgment').selectOption('low_concern');
	await page.locator('#overall-judgment').selectOption('low_concern');
	await page.getByRole('button', { name: 'Complete appraisal' }).click();
	await expect.poll(() => payloads.length).toBe(1);
	expect(payloads[0]).toMatchObject({
		definition_id: 'deepref-rct-generic',
		definition_version: 1,
		responses: { allocation_description: 'yes', outcome_measure_prespecified: true }
	});
	expect(payloads[0]?.evidence).toEqual(
		expect.arrayContaining([
			{
				question_id: 'allocation_description',
				document_id: 'document-1',
				block_id: 'block-1'
			},
			{
				question_id: 'allocation_description',
				document_id: 'document-1',
				block_id: 'block-2'
			}
		])
	);
	expect(payloads[0]?.evidence).toHaveLength(2);
	await page.getByRole('button', { name: 'DeepRef generic qualitative appraisal' }).click();
	await expect(page.locator('#transparency_score')).toBeVisible();
	await expect(page.locator('#reflexivity_note')).toBeVisible();
	await page.locator('#transparency_score').fill('2');
	await page.locator('#reflexivity_note').fill('The methods are clearly described.');
	await page.getByRole('button', { name: 'Add evidence block' }).click();
	await page.locator('#transparency_score-evidence-0').selectOption('block-1');
	await page.locator('#methodological_transparency-judgment').selectOption('adequate');
	await page.locator('#overall-judgment').selectOption('adequate');
	await page.getByRole('button', { name: 'Complete appraisal' }).click();
	await expect.poll(() => payloads.length).toBe(2);
	expect(payloads[1]).toMatchObject({
		definition_id: 'deepref-qualitative-generic',
		definition_version: 1,
		responses: {
			transparency_score: 2,
			reflexivity_note: 'The methods are clearly described.'
		}
	});
	expect(payloads[1]?.evidence).toEqual([
		{ question_id: 'transparency_score', document_id: 'document-1', block_id: 'block-1' }
	]);
});
