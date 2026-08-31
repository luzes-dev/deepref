import { expect, test, type Page } from '@playwright/test';
import type { AiExtractedFieldDto, AiProposalDto } from '$lib/api/generated/models';

const api = 'http://localhost:4173/api';
const project = {
	id: 'project-1',
	name: 'Extraction project',
	description: 'A mocked project',
	default_max_depth: 2,
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z'
};
const study = {
	id: 'study-1',
	project_id: 'project-1',
	title: 'Primary investigation',
	design: 'rct',
	design_label: 'Randomized controlled trial',
	design_context: { physiotherapy: false, exposure: false, prediction_or_ai: false },
	revision: 1,
	reports: [],
	tool_suggestions: [],
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z',
	updated_by_actor_kind: 'user',
	updated_by_actor_id: 'tester'
};
const dependencies = {
	postgresql: { state: 'available', lag: null, backlog: null, oldest_age_seconds: null },
	worker: { state: 'available', lag: 0, backlog: 0, oldest_age_seconds: null }
};
const field = {
	id: 'field-number',
	project_id: 'project-1',
	field_key: 'sample_size',
	label: 'Sample size',
	value_type: 'number',
	required: true,
	version: 2
};
const evidence = {
	report_id: 'report-1',
	document_id: 'document-1',
	document_block_id: 'block-7',
	page: 2,
	parser_version: 'parser.v2',
	content_hash: 'hash-7'
};
const proposalField = {
	field_id: 'field-number',
	field_version: 2,
	kind: 'value',
	rationale: 'The results table reports the enrolled population.',
	source: evidence,
	value: { kind: 'number', value: 42 }
} satisfies AiExtractedFieldDto;

type DecisionMode = 'accept' | 'conflict';
type MockState = {
	fields: Array<typeof field>;
	values: Array<{
		id: string;
		study_id: string;
		field_definition_id: string;
		field_definition_version: number;
		report_id: string;
		source_document_id: string;
		source_block_id: string;
		source_page: number;
		source_parser_version: string;
		source_content_hash: string;
		rationale: string;
		approved_at: string;
		approved_by_actor_id: string;
		approved_by_actor_kind: string;
		value: { kind: 'number'; value: number };
	}>;
	proposalPresent: boolean;
	decisionBody: string;
	createdFieldBody: string;
};

function approvedValue(number: number) {
	return {
		id: 'value-1',
		study_id: 'study-1',
		field_definition_id: 'field-number',
		field_definition_version: 2,
		report_id: 'report-1',
		source_document_id: 'document-1',
		source_block_id: 'block-7',
		source_page: 2,
		source_parser_version: 'parser.v2',
		source_content_hash: 'hash-7',
		rationale: 'The results table reports the enrolled population.',
		approved_at: '2026-01-02T00:00:00Z',
		approved_by_actor_id: 'tester',
		approved_by_actor_kind: 'user',
		value: { kind: 'number' as const, value: number }
	};
}

async function mockExtractionPage(page: Page, decisionMode: DecisionMode): Promise<MockState> {
	const state: MockState = {
		fields: [],
		values: [],
		proposalPresent: false,
		decisionBody: '',
		createdFieldBody: ''
	};

	await page.route(`${api}/health/dependencies`, (route) =>
		route.fulfill({ json: dependencies })
	);
	await page.route(/\/api\/projects(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [project], next_cursor: null } })
	);
	await page.route(`${api}/projects/project-1`, (route) => route.fulfill({ json: project }));
	await page.route(/\/api\/ingestions(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [], next_cursor: null } })
	);
	await page.route(/\/api\/projects\/project-1\/reports(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [], next_cursor: null } })
	);
	await page.route(/\/api\/projects\/project-1\/studies(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [study], next_cursor: null } })
	);
	await page.route(`${api}/projects/project-1/extraction/fields`, async (route) => {
		if (route.request().method() === 'POST') {
			state.createdFieldBody = route.request().postData() ?? '';
			state.fields = [field];
			await route.fulfill({ status: 201, json: field });
			return;
		}
		await route.fulfill({ json: state.fields });
	});
	await page.route(`${api}/projects/project-1/studies/study-1/extraction`, (route) =>
		route.fulfill({ json: state.values })
	);
	await page.route(`${api}/projects/project-1/ai/proposals?**`, (route) => {
		const response = state.proposalPresent
			? {
					items: [
						{
							authority_tier: 'grounded',
							created_at: '2026-01-01T00:00:00Z',
							entity_id: null,
							entity_type: 'study',
							evidence_hash: null,
							expected_revision: null,
							id: 'proposal-1',
							input_hash: 'input-hash',
							model: 'mock-model',
							model_run_id: 'run-1',
							model_version: '1',
							operation: 'propose',
							payload: {
								kind: 'data_extraction',
								study_id: 'study-1',
								fields: [proposalField]
							},
							project_id: 'project-1',
							prompt_hash: 'prompt-hash',
							prompt_version: 'appraisal-extraction-test',
							protocol_version_id: null,
							provider: 'mock-provider',
							resolution_reason: null,
							resolved_at: null,
							resolved_by_actor_id: null,
							resolved_by_actor_kind: null,
							schema_hash: 'schema-hash',
							schema_version: '1',
							status: 'pending',
							target_record_id: null,
							target_report_id: null,
							target_study_id: 'study-1',
							task_kind: 'data_extraction'
						} satisfies AiProposalDto
					],
					next_cursor: null
				}
			: { items: [], next_cursor: null };
		void route.fulfill({ json: response });
	});
	await page.route(`${api}/projects/project-1/studies/study-1/ai/extraction`, async (route) => {
		await route.fulfill({
			status: 202,
			json: {
				id: 'extraction-run',
				project_id: 'project-1',
				definition: 'data_extraction',
				subject: {},
				origin: { kind: 'reviewer_requested' },
				state: { kind: 'queued' },
				created_at: '2026-01-01T00:00:00Z'
			}
		});
	});
	await page.route(`${api}/projects/project-1/review-runs/extraction-run`, async (route) => {
		state.proposalPresent = true;
		await route.fulfill({
			json: {
				id: 'extraction-run',
				project_id: 'project-1',
				definition: 'data_extraction',
				subject: {},
				origin: { kind: 'reviewer_requested' },
				state: { kind: 'completed', proposal_id: 'proposal-1' },
				created_at: '2026-01-01T00:00:00Z',
				finished_at: '2026-01-01T00:00:01Z'
			}
		});
	});
	await page.route(
		`${api}/projects/project-1/ai/proposals/proposal-1/decision`,
		async (route) => {
			state.decisionBody = route.request().postData() ?? '';
			if (decisionMode === 'conflict') {
				await route.fulfill({
					status: 409,
					json: { message: 'Proposal changed elsewhere.' }
				});
				return;
			}
			state.proposalPresent = false;
			state.values = [approvedValue(84.5)];
			await route.fulfill({
				status: 200,
				json: { proposal: { id: 'proposal-1' }, applied_revision: 1 }
			});
		}
	);
	return state;
}

test('creates a field, generates, edits, reviews provenance, and refreshes accepted values', async ({
	page
}) => {
	const state = await mockExtractionPage(page, 'accept');
	await page.goto('/projects/project-1/extraction?study=study-1');

	await expect(page.getByRole('heading', { name: 'Extraction' })).toBeVisible();
	await expect(page.getByRole('link', { name: 'Extraction' })).toBeVisible();
	await expect(page.getByTestId('extraction-proposal-only')).toContainText('Proposal only');

	await page.getByLabel('Field key').fill('sample_size');
	await page.getByLabel('Label').fill('Sample size');
	await page.locator('#extraction-field-type').click();
	await page.getByRole('option', { name: 'number', exact: true }).click();
	await page.getByLabel('Version').fill('2');
	await page.getByLabel('Required field').click();
	await page.getByRole('button', { name: 'Add field' }).click();
	await expect(page.getByTestId('extraction-field-sample_size')).toContainText('v2');
	await expect(state.createdFieldBody).toContain('sample_size');
	await expect(state.createdFieldBody).toContain('"value_type":"number"');
	await expect(state.createdFieldBody).toContain('"required":true');

	await page.getByRole('button', { name: 'Generate proposal' }).click();
	await expect(page.getByTestId('extraction-proposal-editor')).toBeVisible();
	await page.getByRole('button', { name: 'Mark insufficient evidence' }).click();
	await expect(page.getByRole('status')).toContainText('insufficiency cannot be accepted');
	await page.getByTestId('enter-reviewed-value-field-number').click();
	await page.getByLabel('Number value').fill('84.5');
	await expect(page.getByTestId('extraction-evidence-link')).toHaveAttribute(
		'href',
		'/projects/project-1/screening/full-text?report=report-1&page=2&block=block-7'
	);
	await expect(page.getByTestId('extraction-proposal-editor')).toContainText('parser.v2');
	await expect(page.getByTestId('extraction-proposal-editor')).toContainText('hash-7');
	await page.getByRole('button', { name: 'Accept reviewed values' }).click();

	await expect(page.getByTestId('accepted-extraction-values')).toContainText('84.5');
	await expect(page.getByTestId('accepted-extraction-values')).toContainText('approved');
	await expect(state.decisionBody).toContain('"kind":"data_extraction"');
	await expect(state.decisionBody).toContain('"field_id":"field-number"');
	await expect(state.decisionBody).toContain('"field_version":2');
	await expect(state.decisionBody).toContain('84.5');
});

test('keeps a proposal visible when approval conflicts', async ({ page }) => {
	const state = await mockExtractionPage(page, 'conflict');
	await page.goto('/projects/project-1/extraction?study=study-1');
	await page.getByLabel('Field key').fill('sample_size');
	await page.getByLabel('Label').fill('Sample size');
	await page.getByLabel('Version').fill('2');
	await page.getByLabel('Required field').click();
	await page.getByRole('button', { name: 'Add field' }).click();
	await page.getByRole('button', { name: 'Generate proposal' }).click();
	await expect(page.getByTestId('extraction-proposal-editor')).toBeVisible();
	await page.getByRole('button', { name: 'Reject proposal' }).click();
	await expect(page.getByRole('alert').filter({ hasText: 'Review conflict' })).toContainText(
		'Review conflict'
	);
	await expect(page.getByTestId('extraction-proposal-editor')).toBeVisible();
	const rejectBody: unknown = JSON.parse(state.decisionBody);
	expect(rejectBody).toEqual({
		decision: 'reject',
		reason: 'Human reviewer rejected the data extraction proposal.'
	});
	expect(rejectBody).not.toHaveProperty('reviewed_payload');
});
