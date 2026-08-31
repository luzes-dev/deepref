import { expect, test, type Page } from '@playwright/test';
import type { AiProposalDto } from '$lib/api/generated/models';

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
	await page.route(/\/api\/projects(?:\?.*)?$/, async (route) =>
		route.fulfill({ json: { items: [project], next_cursor: null } })
	);
	await page.route(/\/api\/projects\/project-1(?:\?.*)?$/, async (route) =>
		route.fulfill({ json: project })
	);
	await page.route(/\/api\/ingestions(?:\?.*)?$/, async (route) =>
		route.fulfill({ json: { items: [], next_cursor: null } })
	);
	await page.route(/\/api\/projects\/project-1\/reports(?:\?.*)?$/, async (route) =>
		route.fulfill({ json: { items: [report], next_cursor: null } })
	);
	await page.route(/\/api\/projects\/project-1\/ai\/proposals(?:\?.*)?$/, async (route) =>
		route.fulfill({ json: { items: [], next_cursor: null } })
	);
	await page.route(/\/api\/projects\/project-1\/appraisal-definitions(?:\?.*)?$/, async (route) =>
		route.fulfill({ json: definitions })
	);
	await page.route(
		/\/api\/projects\/project-1\/reports\/report-1\/documents(?:\?.*)?$/,
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
		/\/api\/projects\/project-1\/reports\/report-1\/documents\/document-1\/blocks(?:\?.*)?$/,
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
		/\/api\/projects\/project-1\/reports\/report-1\/appraisals(?:\?.*)?$/,
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

test('reviews an edited AI appraisal pre-fill with evidence navigation and decision refreshes', async ({
	page
}) => {
	await mockShell(page);
	const evidenceHash = 'a'.repeat(64);
	const proposalPayload = {
		kind: 'appraisal_prefill' as const,
		report_id: report.report_id,
		definition_id: 'deepref-rct-generic',
		definition_version: 1,
		answers: [
			{
				question_id: 'allocation_description',
				answer: { kind: 'enum' as const, value: 'yes' },
				rationale: 'The report describes the allocation process.',
				evidence: [
					{
						document_id: 'document-1',
						document_block_id: 'block-1',
						page: 1,
						parser_version: 'parser-1',
						content_hash: evidenceHash
					}
				]
			},
			{
				question_id: 'outcome_measure_prespecified',
				answer: { kind: 'boolean' as const, value: true },
				rationale: 'The outcome measure was prespecified.',
				evidence: []
			}
		],
		domain_judgments: { allocation: 'low_concern', outcome_reporting: 'low_concern' },
		overall_judgment: 'low_concern'
	};
	let proposalNumber = 0;
	let pendingProposal: AiProposalDto | null = null;
	let historyReads = 0;
	let completeCalls = 0;
	const decisions: unknown[] = [];
	const appraisalHistory: Record<string, unknown>[] = [];

	function createProposal(): AiProposalDto {
		proposalNumber += 1;
		return {
			authority_tier: 'grounded',
			created_at: '2026-01-01T00:00:00Z',
			entity_id: null,
			entity_type: 'report',
			evidence_hash: evidenceHash,
			expected_revision: null,
			id: `proposal-${proposalNumber}`,
			input_hash: `input-${proposalNumber}`,
			model: 'mock-model',
			model_run_id: `run-${proposalNumber}`,
			model_version: '1',
			operation: 'appraisal_prefill',
			payload: proposalPayload,
			project_id: project.id,
			prompt_hash: 'prompt-hash',
			prompt_version: '1',
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
			target_report_id: report.report_id,
			target_study_id: null,
			task_kind: 'appraisal_prefill'
		};
	}

	await page.route(
		/http:\/\/localhost:4173\/api\/projects\/project-1\/ai\/proposals(?:\?.*)?$/,
		async (route) =>
			await route.fulfill({
				json: { items: pendingProposal ? [pendingProposal] : [], next_cursor: null }
			})
	);
	await page.route(
		'http://localhost:4173/api/projects/project-1/reports/report-1/ai/appraisal-prefill',
		async (route) => {
			pendingProposal = createProposal();
			await route.fulfill({
				status: 202,
				json: {
					id: 'appraisal-run',
					project_id: 'project-1',
					definition: 'appraisal_prefill',
					subject: {},
					origin: { kind: 'reviewer_requested' },
					state: { kind: 'queued' },
					created_at: '2026-01-01T00:00:00Z'
				}
			});
		}
	);
	await page.route(
		'http://localhost:4173/api/projects/project-1/review-runs/appraisal-run',
		async (route) => {
			await route.fulfill({
				json: {
					id: 'appraisal-run',
					project_id: 'project-1',
					definition: 'appraisal_prefill',
					subject: {},
					origin: { kind: 'reviewer_requested' },
					state: { kind: 'completed', proposal_id: pendingProposal?.id ?? 'proposal-1' },
					created_at: '2026-01-01T00:00:00Z',
					finished_at: '2026-01-01T00:00:01Z'
				}
			});
		}
	);
	await page.route(
		/http:\/\/localhost:4173\/api\/projects\/project-1\/ai\/proposals\/proposal-\d+\/decision$/,
		async (route) => {
			const body: unknown = route.request().postDataJSON();
			decisions.push(body);
			const resolvedProposal = pendingProposal ?? createProposal();
			const isAccept =
				typeof body === 'object' &&
				body !== null &&
				'decision' in body &&
				body.decision === 'accept';
			if (isAccept) {
				appraisalHistory.push({
					id: 'assessment-ai-1',
					project_id: project.id,
					report_id: report.report_id,
					definition_id: proposalPayload.definition_id,
					definition_version: proposalPayload.definition_version,
					responses: {
						allocation_description: 'yes',
						outcome_measure_prespecified: false
					},
					judgments: proposalPayload.domain_judgments,
					evidence: proposalPayload.answers[0].evidence,
					actor_kind: 'user',
					actor_id: 'tester',
					completed_at: '2026-01-02T00:00:00Z',
					created_at: '2026-01-02T00:00:00Z'
				});
			}
			pendingProposal = null;
			await route.fulfill({
				json: {
					proposal: { ...resolvedProposal, status: isAccept ? 'accepted' : 'rejected' },
					applied_revision: 1
				}
			});
		}
	);
	await page.route(
		'http://localhost:4173/api/projects/project-1/reports/report-1/appraisals',
		async (route) => {
			if (route.request().method() === 'POST') completeCalls += 1;
			if (route.request().method() === 'GET') {
				historyReads += 1;
				await route.fulfill({ json: appraisalHistory });
				return;
			}
			await route.fallback();
		}
	);

	await page.goto(
		'/projects/project-1/appraisal?report=report-1&definition=deepref-rct-generic&definition_version=1'
	);
	await page.getByTestId('generate-ai-prefill').click();
	await expect(page.getByTestId('ai-prefill-proposal')).toBeVisible();
	await expect(page.getByTestId('ai-answer-allocation_description')).toContainText(
		'Suggested answer: yes'
	);
	const evidenceLink = page.getByTestId('ai-evidence-link-allocation_description').first();
	await expect(evidenceLink).toHaveAttribute(
		'href',
		'/projects/project-1/screening/full-text?report=report-1&page=1&block=block-1'
	);
	await evidenceLink.click();
	await expect(page).toHaveURL(
		/projects\/project-1\/screening\/full-text\?report=report-1&page=1&block=block-1/
	);
	await page.goto(
		'/projects/project-1/appraisal?report=report-1&definition=deepref-rct-generic&definition_version=1'
	);
	await expect(page.getByTestId('ai-prefill-proposal')).toBeVisible();

	await page.locator('#outcome_measure_prespecified').uncheck();
	await page.locator('#allocation_description-evidence-0').selectOption('block-2');
	await page.getByRole('button', { name: 'Accept reviewed AI pre-fill' }).click();
	await expect.poll(() => decisions.length).toBe(1);
	const acceptedBody = decisions[0];
	expect(acceptedBody).toMatchObject({
		decision: 'accept',
		reviewed_payload: {
			kind: 'appraisal_prefill',
			report_id: 'report-1',
			definition_id: 'deepref-rct-generic',
			definition_version: 1,
			answers: [
				{
					question_id: 'allocation_description',
					evidence: [{ document_block_id: 'block-2', content_hash: 'b'.repeat(64) }]
				},
				{
					question_id: 'outcome_measure_prespecified',
					answer: { kind: 'boolean', value: false }
				}
			]
		}
	});
	await expect(page.getByText('assessment-ai-1')).toBeVisible();
	await expect.poll(() => historyReads).toBeGreaterThan(1);
	expect(completeCalls).toBe(0);
	await expect(page.getByText('No pending AI pre-fill')).toBeVisible();

	await page.getByTestId('generate-ai-prefill').click();
	await expect(page.getByTestId('ai-prefill-proposal')).toBeVisible();
	await page.getByTestId('reject-ai-prefill').click();
	await expect.poll(() => decisions.length).toBe(2);
	expect(decisions[1]).toEqual({
		decision: 'reject',
		reason: 'Human reviewer rejected the AI appraisal prefill.'
	});
	if (decisions[1] && typeof decisions[1] === 'object') {
		expect('reviewed_payload' in decisions[1]).toBe(false);
	}

	await page.getByTestId('generate-ai-prefill').click();
	await expect(page.getByTestId('ai-prefill-proposal')).toBeVisible();
	await page.route(
		'http://localhost:4173/api/projects/project-1/ai/proposals/proposal-3/decision',
		async (route) => await route.fulfill({ status: 409, json: { message: 'revision changed' } })
	);
	await page.getByTestId('reject-ai-prefill').click();
	await expect(page.getByRole('alert')).toContainText('stale');
	await expect(page.getByTestId('ai-prefill-proposal')).toBeVisible();
});
