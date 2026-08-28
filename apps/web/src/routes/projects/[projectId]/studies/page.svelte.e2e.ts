import { expect, test, type Page } from '@playwright/test';
import type { AiProposalDto } from '$lib/api/generated/models';

const project = {
	id: 'project-1',
	name: 'Studies project',
	description: 'A mocked project',
	default_max_depth: 2,
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z'
};

const dependencies = {
	postgresql: { state: 'available', lag: null, backlog: null, oldest_age_seconds: null },
	worker: { state: 'available', lag: 0, backlog: 0, oldest_age_seconds: null }
};

async function mockProjectShell(page: Page): Promise<void> {
	await page.route('http://localhost:4173/api/health/dependencies', async (route) => {
		await route.fulfill({ json: dependencies });
	});
	await page.route(/http:\/\/localhost:4173\/api\/projects(?:\?.*)?$/, async (route) => {
		await route.fulfill({ json: { items: [project], next_cursor: null } });
	});
	await page.route('http://localhost:4173/api/projects/project-1', async (route) => {
		await route.fulfill({ json: project });
	});
	await page.route(/http:\/\/localhost:4173\/api\/ingestions(?:\?.*)?$/, async (route) => {
		await route.fulfill({ json: { items: [], next_cursor: null } });
	});
}

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

test('groups, unassigns, and preserves study history', async ({ page }) => {
	await mockProjectShell(page);
	let study = {
		id: 'study-1',
		project_id: project.id,
		title: 'One investigation',
		design: null,
		design_label: null,
		design_context: { physiotherapy: false, exposure: false, prediction_or_ai: false },
		revision: 0,
		reports: [] as Array<Record<string, unknown>>,
		tool_suggestions: [],
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-01T00:00:00Z',
		updated_by_actor_kind: 'user',
		updated_by_actor_id: 'tester'
	};
	let sourceStudy = {
		...study,
		id: 'study-2',
		title: 'Source investigation',
		revision: 1,
		reports: [{ ...report, role: 'report_of_study', assigned_at: '2026-01-01T00:00:00Z' }]
	};
	const history: Array<Record<string, unknown>> = [];
	let membershipReads = 0;
	let movePayload: Record<string, unknown> | undefined;
	await page.route(
		/http:\/\/localhost:4173\/api\/projects\/project-1\/reports(?:\?.*)?$/,
		async (route) => {
			await route.fulfill({ json: { items: [report], next_cursor: null } });
		}
	);
	await page.route(
		/http:\/\/localhost:4173\/api\/projects\/project-1\/studies(?:\/.*)?(?:\?.*)?$/,
		async (route) => {
			const path = new URL(route.request().url()).pathname;
			if (path.endsWith('/history')) {
				await route.fulfill({ json: history });
			} else if (route.request().method() === 'GET') {
				await route.fulfill({
					json: path.endsWith('study-1')
						? study
						: path.endsWith('study-2')
							? sourceStudy
							: { items: [study, sourceStudy], next_cursor: null }
				});
			} else {
				await route.continue();
			}
		}
	);
	await page.route(
		'http://localhost:4173/api/projects/project-1/reports/report-1/study',
		async (route) => {
			if (route.request().method() === 'GET') {
				membershipReads += 1;
				if (sourceStudy.reports.length === 0) {
					await route.fulfill({ status: 204, body: '' });
				} else {
					await route.fulfill({
						json: {
							study_id: sourceStudy.id,
							role: sourceStudy.reports[0].role,
							study_revision: sourceStudy.revision,
							study: sourceStudy
						}
					});
				}
				return;
			}
			const body = route.request().postDataJSON() as { study_id?: string; role?: string };
			if (body.study_id) movePayload = body;
			if (body.study_id) {
				study = {
					...study,
					revision: study.revision + 1,
					reports: [{ ...report, role: body.role, assigned_at: '2026-01-01T00:00:00Z' }]
				};
				sourceStudy = { ...sourceStudy, revision: sourceStudy.revision + 1, reports: [] };
				history.push({
					id: `event-${history.length + 1}`,
					study_id: study.id,
					report_id: report.report_id,
					event_type: 'report_assigned',
					before_revision: study.revision - 1,
					result_revision: study.revision,
					actor_id: 'tester',
					actor_kind: 'user',
					created_at: '2026-01-01T00:00:00Z'
				});
				await route.fulfill({ json: study });
			} else {
				study = { ...study, revision: study.revision + 1, reports: [] };
				history.push({
					id: `event-${history.length + 1}`,
					study_id: study.id,
					report_id: report.report_id,
					event_type: 'report_unassigned',
					before_revision: study.revision - 1,
					result_revision: study.revision,
					actor_id: 'tester',
					actor_kind: 'user',
					created_at: '2026-01-01T00:00:00Z'
				});
				await route.fulfill({ json: study });
			}
		}
	);

	await page.goto('/projects/project-1/studies?study=study-1');
	await expect(page.getByRole('heading', { name: 'Studies' })).toBeVisible();
	await page.locator('#study-report').click();
	await page.getByRole('option', { name: 'Primary trial report' }).click();
	await expect.poll(() => membershipReads).toBeGreaterThan(0);
	await expect(page.getByRole('button', { name: 'Assign / move' })).toBeEnabled();
	await page.getByRole('button', { name: 'Assign / move' }).click();
	await expect.poll(() => movePayload?.expected_previous_study_revision).toBe(1);
	await expect(page.getByText('Primary trial report', { exact: true })).toBeVisible();
	await page.getByRole('button', { name: 'Unassign' }).click();
	await expect(page.getByText('No reports assigned yet.', { exact: true })).toBeVisible();
	await expect(page.getByText('report_unassigned', { exact: true })).toBeVisible();
});

test('reviews study grouping proposals with typed provenance and refreshes after decisions', async ({
	page
}) => {
	await mockProjectShell(page);

	let study = {
		id: 'study-1',
		project_id: project.id,
		title: 'One investigation',
		design: null,
		design_label: null,
		design_context: { physiotherapy: false, exposure: false, prediction_or_ai: false },
		revision: 2,
		reports: [] as Array<{
			report_id: string;
			title: string;
			abstract_text: string | null;
			publication_year: number;
			role: string;
			assigned_at: string;
		}>,
		tool_suggestions: [],
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-01T00:00:00Z',
		updated_by_actor_kind: 'user',
		updated_by_actor_id: 'tester'
	};
	const groupingProposal = (id: string): AiProposalDto => ({
		authority_tier: 'ai_proposal',
		created_at: '2026-01-01T00:00:00Z',
		entity_id: null,
		entity_type: 'report',
		evidence_hash: 'evidence-hash',
		expected_revision: null,
		id,
		input_hash: 'input-hash',
		model: 'test-model',
		model_run_id: 'run-1',
		model_version: '1',
		operation: 'study_grouping',
		payload: {
			choice: { kind: 'existing_study', study_id: 'study-1', expected_revision: 2 },
			expected_previous_study_id: null,
			expected_previous_study_revision: null,
			kind: 'study_grouping',
			provenance: [
				{
					content_hash: 'report-title-hash-fully-visible',
					field: 'title',
					kind: 'report_metadata',
					report_id: report.report_id
				},
				{
					content_hash: 'study-title-hash-fully-visible',
					field: 'title',
					kind: 'study_metadata',
					study_id: 'study-1'
				}
			],
			rationale: 'The report title and publication year match the existing investigation.',
			report_id: report.report_id,
			uncertainties: []
		},
		project_id: project.id,
		prompt_hash: 'prompt-hash',
		prompt_version: 'study-grouping-v1',
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
		task_kind: 'study_grouping'
	});

	let pendingProposal: AiProposalDto | undefined;
	let generationRequests = 0;
	let proposalListRequests = 0;
	let reportsReady = false;
	type GroupingDecisionBody = {
		decision: 'accept' | 'reject';
		reason: string;
		reviewed_payload?: never;
	};
	const decisionBodies: GroupingDecisionBody[] = [];

	await page.route(
		/http:\/\/localhost:4173\/api\/projects\/project-1\/reports(?:\?.*)?$/,
		async (route) => {
			reportsReady = true;
			await route.fulfill({ json: { items: [report], next_cursor: null } });
		}
	);
	await page.route(
		/http:\/\/localhost:4173\/api\/projects\/project-1\/studies(?:\/.*)?(?:\?.*)?$/,
		async (route) => {
			const url = new URL(route.request().url());
			const path = url.pathname;
			if (path.endsWith('/history')) {
				await route.fulfill({ json: [] });
			} else if (route.request().method() === 'GET') {
				await route.fulfill({
					json: path.endsWith('/study-1') ? study : { items: [study], next_cursor: null }
				});
			} else {
				await route.continue();
			}
		}
	);
	await page.route(
		'http://localhost:4173/api/projects/project-1/reports/report-1/study',
		async (route) => {
			await route.fulfill({ status: 204, body: '' });
		}
	);
	await page.route(
		/http:\/\/localhost:4173\/api\/projects\/project-1\/ai\/proposals(?:\?.*)?$/,
		async (route) => {
			proposalListRequests += 1;
			await route.fulfill({
				json: {
					items: pendingProposal ? [pendingProposal] : [],
					next_cursor: null
				}
			});
		}
	);
	await page.route(
		'http://localhost:4173/api/projects/project-1/reports/report-1/ai/study-grouping',
		async (route) => {
			generationRequests += 1;
			pendingProposal = groupingProposal(`grouping-${generationRequests}`);
			await route.fulfill({ json: pendingProposal });
		}
	);
	await page.route(
		/http:\/\/localhost:4173\/api\/projects\/project-1\/ai\/proposals\/[^/]+\/decision$/,
		async (route) => {
			const body: GroupingDecisionBody = route.request().postDataJSON();
			decisionBodies.push(body);
			if (body.decision === 'accept') {
				study = { ...study, revision: 3 };
				pendingProposal = undefined;
				await route.fulfill({
					json: {
						applied_revision: 3,
						proposal: { ...groupingProposal('grouping-1'), status: 'accepted' }
					}
				});
				return;
			}
			await route.fulfill({
				status: 409,
				json: { code: 'stale_revision', message: 'Study revision is stale.' }
			});
		}
	);

	await page.goto('/projects/project-1/studies?study=study-1&report=report-1');
	await expect.poll(() => reportsReady).toBe(true);
	await expect(page.getByRole('button', { name: 'Suggest study group' })).toBeVisible();
	await page.getByRole('button', { name: 'Suggest study group' }).click();
	await expect(page.getByTestId('study-grouping-choice')).toContainText('Existing study');
	await expect(page.getByTestId('study-grouping-provenance')).toContainText(
		'report-title-hash-fully-visible'
	);
	await expect(page.getByTestId('study-grouping-provenance')).toContainText(
		'study-title-hash-fully-visible'
	);

	const listRequestsBeforeAccept = proposalListRequests;
	await page.getByRole('button', { name: 'Accept and apply' }).click();
	await expect.poll(() => decisionBodies.length).toBe(1);
	await expect.poll(() => proposalListRequests).toBeGreaterThan(listRequestsBeforeAccept);
	await expect(decisionBodies[0]).toEqual({
		decision: 'accept',
		reason: 'Human reviewer accepted study grouping suggestion.'
	});
	await expect(page.getByText('No pending grouping suggestion', { exact: true })).toBeVisible();
	await expect(page.getByText('Revision 3 · changes are audited and reversible')).toBeVisible();

	await page.getByRole('button', { name: 'Suggest study group' }).click();
	await expect(page.getByTestId('study-grouping-choice')).toBeVisible();
	await page.getByRole('button', { name: 'Reject' }).click();
	await expect.poll(() => decisionBodies.length).toBe(2);
	await expect(decisionBodies[1]).toEqual({
		decision: 'reject',
		reason: 'Human reviewer rejected study grouping suggestion.'
	});
	await expect(page.getByRole('alert')).toContainText('Study data changed elsewhere');
});

test('reviews study classification proposals without calling manual classification', async ({
	page
}) => {
	await mockProjectShell(page);
	let study = {
		id: 'study-1',
		project_id: project.id,
		title: 'One investigation',
		design: null as string | null,
		design_label: null as string | null,
		design_context: { physiotherapy: false, exposure: false, prediction_or_ai: false },
		revision: 2,
		reports: [],
		tool_suggestions: [],
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-01T00:00:00Z',
		updated_by_actor_kind: 'user',
		updated_by_actor_id: 'tester'
	};
	const classificationProposal: AiProposalDto = {
		authority_tier: 'ai_proposal',
		created_at: '2026-01-01T00:00:00Z',
		entity_id: 'study-1',
		entity_type: 'study_classification',
		evidence_hash: 'evidence-hash',
		expected_revision: 2,
		id: 'classification-1',
		input_hash: 'input-hash',
		model: 'test-model',
		model_run_id: 'run-1',
		model_version: '1',
		operation: 'study_design_classification_suggestion',
		payload: {
			evidence: [
				{
					content_hash:
						'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
					field: 'title',
					kind: 'study_metadata',
					study_id: 'study-1'
				},
				{
					content_hash:
						'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
					field: 'abstract',
					kind: 'report_metadata',
					report_id: 'report-1'
				}
			],
			kind: 'classification',
			rationale: 'The report describes randomized allocation for this investigation.',
			study_id: 'study-1',
			suggested_design: 'rct',
			uncertainties: ['The allocation wording should be checked against the full text.']
		},
		project_id: project.id,
		prompt_hash: 'prompt-hash',
		prompt_version: 'study-design-v1',
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
		task_kind: 'study_design_classification'
	};
	let pendingProposal: AiProposalDto | undefined = classificationProposal;
	let proposalQueryRequests = 0;
	let manualClassifyRequests = 0;
	const decisionBodies: Array<Record<string, unknown>> = [];
	const history: Array<Record<string, unknown>> = [];

	await page.route(
		/http:\/\/localhost:4173\/api\/projects\/project-1\/reports(?:\?.*)?$/,
		async (route) => {
			await route.fulfill({ json: { items: [report], next_cursor: null } });
		}
	);
	await page.route(
		/http:\/\/localhost:4173\/api\/projects\/project-1\/studies(?:\/.*)?(?:\?.*)?$/,
		async (route) => {
			const path = new URL(route.request().url()).pathname;
			if (path.endsWith('/history')) {
				await route.fulfill({ json: history });
			} else if (route.request().method() === 'GET') {
				await route.fulfill({
					json: path.endsWith('/study-1') ? study : { items: [study], next_cursor: null }
				});
			} else {
				await route.continue();
			}
		}
	);
	await page.route(
		'http://localhost:4173/api/projects/project-1/studies/study-1/classification',
		async (route) => {
			manualClassifyRequests += 1;
			await route.fulfill({
				status: 500,
				json: { message: 'manual classification must not run' }
			});
		}
	);
	await page.route(
		/http:\/\/localhost:4173\/api\/projects\/project-1\/ai\/proposals(?:\?.*)?$/,
		async (route) => {
			const params = new URL(route.request().url()).searchParams;
			if (params.get('task_kind') !== 'study_design_classification') {
				await route.fulfill({ json: { items: [], next_cursor: null } });
				return;
			}
			proposalQueryRequests += 1;
			expect(params.get('status')).toBe('pending');
			expect(params.get('target_study_id')).toBe('study-1');
			expect(params.get('limit')).toBe('1');
			await route.fulfill({
				json: { items: pendingProposal ? [pendingProposal] : [], next_cursor: null }
			});
		}
	);
	await page.route(
		/http:\/\/localhost:4173\/api\/projects\/project-1\/ai\/proposals\/[^/]+\/decision$/,
		async (route) => {
			const body = route.request().postDataJSON() as Record<string, unknown>;
			decisionBodies.push(body);
			expect(body).toEqual({
				decision: 'accept',
				reason: 'Human reviewer accepted study design classification suggestion.'
			});
			pendingProposal = undefined;
			study = { ...study, design: 'rct', design_label: 'rct', revision: 3 };
			history.push({
				id: 'history-1',
				study_id: 'study-1',
				report_id: null,
				event_type: 'study_classified',
				before_revision: 2,
				result_revision: 3,
				actor_id: 'local-user',
				actor_kind: 'user',
				created_at: '2026-01-01T00:00:00Z'
			});
			await route.fulfill({
				json: {
					applied_revision: 3,
					proposal: { ...classificationProposal, status: 'accepted' }
				}
			});
		}
	);

	await page.goto('/projects/project-1/studies?study=study-1');
	await expect.poll(() => proposalQueryRequests).toBeGreaterThan(0);
	await expect(page.getByTestId('study-classification-suggestion')).toContainText(
		'Randomized controlled trial'
	);
	await expect(page.getByTestId('study-classification-suggestion')).toContainText('rct');
	await expect(page.getByTestId('study-classification-provenance')).toContainText(
		'Study One investigation · study-1'
	);
	await expect(page.getByTestId('study-classification-provenance')).toContainText(
		'Report report-1'
	);
	await expect(page.getByTestId('study-classification-provenance')).toContainText(
		'content hash: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
	);
	await expect(page.getByTestId('study-classification-provenance')).toContainText(
		'content hash: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'
	);
	await expect(page.getByTestId('study-classification-uncertainties')).toContainText(
		'allocation wording'
	);
	await expect(page.getByTestId('study-classification-accept')).toBeEnabled();
	await page.getByTestId('study-classification-accept').click();
	await expect.poll(() => decisionBodies.length).toBe(1);
	await expect.poll(() => proposalQueryRequests).toBeGreaterThan(1);
	await expect(
		page.getByText('No pending classification suggestion', { exact: true })
	).toBeVisible();
	await expect(page.getByText('Revision 3 · changes are audited and reversible')).toBeVisible();
	await expect(page.getByText('study_classified', { exact: true })).toBeVisible();
	await expect.poll(() => manualClassifyRequests).toBe(0);
});

test('rejects abstention classification proposals while keeping accept disabled', async ({
	page
}) => {
	await mockProjectShell(page);
	const study = {
		id: 'study-1',
		project_id: project.id,
		title: 'One investigation',
		design: null,
		design_label: null,
		design_context: { physiotherapy: false, exposure: false, prediction_or_ai: false },
		revision: 2,
		reports: [],
		tool_suggestions: [],
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-01T00:00:00Z',
		updated_by_actor_kind: 'user',
		updated_by_actor_id: 'tester'
	};
	const abstentionProposal: AiProposalDto = {
		authority_tier: 'ai_proposal',
		created_at: '2026-01-01T00:00:00Z',
		entity_id: 'study-1',
		entity_type: 'study_classification',
		evidence_hash: 'evidence-hash',
		expected_revision: 2,
		id: 'classification-abstention',
		input_hash: 'input-hash',
		model: 'test-model',
		model_run_id: 'run-1',
		model_version: '1',
		operation: 'study_design_classification_suggestion',
		payload: {
			evidence: [
				{
					content_hash:
						'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc',
					field: 'title',
					kind: 'study_metadata',
					study_id: 'study-1'
				}
			],
			kind: 'classification',
			rationale: 'The available metadata does not identify a closed design.',
			study_id: 'study-1',
			suggested_design: null,
			uncertainties: ['The study design remains unclear.']
		},
		project_id: project.id,
		prompt_hash: 'prompt-hash',
		prompt_version: 'study-design-v1',
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
		task_kind: 'study_design_classification'
	};
	let pendingProposal: AiProposalDto | undefined = abstentionProposal;
	let rejectCount = 0;
	let manualClassifyRequests = 0;
	await page.route(/\/api\/projects\/project-1\/reports(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [], next_cursor: null } })
	);
	await page.route(/\/api\/projects\/project-1\/studies(?:\/.*)?(?:\?.*)?$/, async (route) => {
		const path = new URL(route.request().url()).pathname;
		if (path.endsWith('/history')) await route.fulfill({ json: [] });
		else if (route.request().method() === 'GET') {
			await route.fulfill({
				json: path.endsWith('/study-1') ? study : { items: [study], next_cursor: null }
			});
		} else await route.continue();
	});
	await page.route(
		'http://localhost:4173/api/projects/project-1/studies/study-1/classification',
		async (route) => {
			manualClassifyRequests += 1;
			await route.fulfill({
				status: 500,
				json: { message: 'manual classification must not run' }
			});
		}
	);
	await page.route(/\/api\/projects\/project-1\/ai\/proposals(?:\?.*)?$/, async (route) => {
		const params = new URL(route.request().url()).searchParams;
		if (params.get('task_kind') === 'study_design_classification') {
			expect(params.get('target_study_id')).toBe('study-1');
			await route.fulfill({
				json: { items: pendingProposal ? [pendingProposal] : [], next_cursor: null }
			});
		} else await route.fulfill({ json: { items: [], next_cursor: null } });
	});
	await page.route(
		/\/api\/projects\/project-1\/ai\/proposals\/[^/]+\/decision$/,
		async (route) => {
			const body = route.request().postDataJSON() as { decision: string };
			expect(body.decision).toBe('reject');
			rejectCount += 1;
			pendingProposal = undefined;
			await route.fulfill({
				json: {
					applied_revision: null,
					proposal: { ...abstentionProposal, status: 'rejected' }
				}
			});
		}
	);

	await page.goto('/projects/project-1/studies?study=study-1');
	await expect(page.getByTestId('study-classification-suggestion')).toContainText('Abstention');
	await expect(page.getByTestId('study-classification-accept')).toBeDisabled();
	await expect(page.getByTestId('study-classification-reject')).toBeEnabled();
	await page.getByTestId('study-classification-reject').click();
	await expect.poll(() => rejectCount).toBe(1);
	await expect(
		page.getByText('No pending classification suggestion', { exact: true })
	).toBeVisible();
	await expect.poll(() => manualClassifyRequests).toBe(0);
});
