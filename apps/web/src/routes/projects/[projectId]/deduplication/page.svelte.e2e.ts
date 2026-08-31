import { expect, test, type Page } from '@playwright/test';

const project = {
	id: 'project-1',
	name: 'Deduplication project',
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
	await page.route(
		/http:\/\/localhost:4173\/api\/projects\/project-1\/reports(?:\?.*)?$/,
		async (route) => {
			await route.fulfill({ json: { items: [], next_cursor: null } });
		}
	);
	await page.route(/http:\/\/localhost:4173\/api\/ingestions(?:\?.*)?$/, async (route) => {
		await route.fulfill({ json: { items: [], next_cursor: null } });
	});
}

const proposal = {
	id: 'proposal-1',
	project_id: 'project-1',
	record_id: 'record-1',
	proposal_kind: 'fuzzy',
	status: 'pending',
	revision: 0,
	score: 0.91,
	title_similarity: 0.96,
	year_match: true,
	first_author_similarity: 0.88,
	exact_identifier_match: false,
	conflicting_identifier: false,
	source_title: 'Effects of exercise on sleep quality in adult',
	source_abstract: 'A source abstract.',
	source_year: 2024,
	source_authors: { family: 'Smith' },
	source_identifiers: { doi: '10.5555/source' },
	candidate_report_id: 'report-1',
	candidate_title: 'Effects of exercise on sleep quality in adults',
	candidate_year: 2024,
	candidate_authors: { family: 'Smith' },
	candidate_identifiers: { doi: '10.5555/candidate' },
	metadata: { shortlist: 'pg_trgm', threshold: 0.82 },
	created_at: '2026-01-01T00:00:00Z'
};

test('navigates to deduplication and resolves a proposal with an auditable decision', async ({
	page
}) => {
	await mockProjectShell(page);
	let remaining = [proposal];
	const decisions: Array<Record<string, unknown>> = [];

	await page.route(
		'http://localhost:4173/api/projects/project-1/deduplication/proposals?limit=100&status=pending',
		async (route) => {
			if (route.request().method() === 'GET') {
				await route.fulfill({ json: { items: remaining, next_cursor: null } });
				return;
			}
			await route.continue();
		}
	);
	await page.route(
		'http://localhost:4173/api/projects/project-1/deduplication/proposals/proposal-1/decision',
		async (route) => {
			expect(route.request().method()).toBe('POST');
			const body = route.request().postDataJSON();
			decisions.push(body);
			remaining = [];
			await route.fulfill({
				json: {
					record_id: 'record-1',
					prior_report_id: 'report-1',
					resolved_report_id: 'report-1',
					action: 'accept_proposal'
				}
			});
		}
	);

	await page.goto('/projects/project-1/deduplication');
	await expect(page.getByRole('heading', { name: 'Resolve duplicate records' })).toBeVisible();
	await expect(
		page
			.getByLabel('Source record')
			.getByText('Effects of exercise on sleep quality in adult', {
				exact: true
			})
	).toBeVisible();
	await page.getByRole('button', { name: 'Accept candidate' }).click();
	await expect(page.getByText('No pending proposals', { exact: true })).toBeVisible();
	await expect.poll(() => decisions.length).toBe(1);
	expect(decisions[0]).toMatchObject({
		decision: 'accept',
		actor_kind: 'user',
		reason: 'Manual deduplication decision: accept'
	});
});

test('shows a loading state and an empty queue', async ({ page }) => {
	await mockProjectShell(page);
	await page.route(
		'http://localhost:4173/api/projects/project-1/deduplication/proposals?limit=100&status=pending',
		async (route) => {
			await new Promise((resolve) => setTimeout(resolve, 250));
			await route.fulfill({ json: { items: [], next_cursor: null } });
		}
	);

	await page.goto('/projects/project-1/deduplication');
	await expect(page.getByLabel('Loading deduplication proposals')).toBeVisible();
	await expect(page.getByText('No pending proposals', { exact: true })).toBeVisible();
});

test('does not offer create-new for identifier conflicts', async ({ page }) => {
	await mockProjectShell(page);
	const conflictProposal = {
		...proposal,
		id: 'conflict-proposal-1',
		proposal_kind: 'conflict',
		conflicting_identifier: true,
		source_title: 'Conflicting identifier source'
	};
	await page.route(
		'http://localhost:4173/api/projects/project-1/deduplication/proposals?limit=100&status=pending',
		async (route) => {
			await route.fulfill({ json: { items: [conflictProposal], next_cursor: null } });
		}
	);

	await page.goto('/projects/project-1/deduplication');
	await expect(page.getByRole('heading', { name: 'Resolve duplicate records' })).toBeVisible();
	await expect(page.getByText('Identifier conflict', { exact: true })).toBeVisible();
	await expect(page.getByRole('button', { name: 'Create new report' })).toHaveCount(0);
});

test('reviews a grounded duplicate AI proposal before applying it', async ({ page }) => {
	await mockProjectShell(page);
	await page.route(
		new RegExp('/api/projects/project-1/deduplication/proposals(?:\\?.*)?$'),
		async (route) => {
			await route.fulfill({ json: { items: [proposal], next_cursor: null } });
		}
	);
	const aiProposal = {
		id: 'ai-dedupe-proposal-1',
		project_id: 'project-1',
		task_kind: 'duplicate_candidate_detection',
		status: 'pending',
		target_record_id: 'record-1',
		target_report_id: 'report-1',
		protocol_version_id: null,
		expected_revision: 0,
		provider: 'deterministic-fixture',
		model: 'fixture-model',
		model_version: 'fixture-v1',
		prompt_version: 'dedupe.v1',
		schema_version: 'dedupe.schema.v1',
		model_run_id: 'run-dedupe-1',
		operation: 'dedupe_suggestion',
		entity_type: 'dedupe_record',
		entity_id: 'record-1',
		authority_tier: 'workflow_suggestion',
		created_at: '2026-01-01T00:00:00Z',
		payload: {
			kind: 'duplicate',
			task_kind: 'duplicate_candidate_detection',
			candidate: { source_record_id: 'record-1', candidate_report_id: 'report-1' },
			decision: 'match',
			rationale: [
				{ code: 'stable_title', explanation: 'Stable title and author signals agree.' }
			],
			signals: [{ kind: 'title_similarity', similarity: 0.96, supports_match: true }],
			provenance: [
				{
					entity_type: 'record',
					entity_id: 'record-1',
					field: 'title',
					content_hash: 'a'.repeat(64)
				},
				{
					entity_type: 'report',
					entity_id: 'report-1',
					field: 'title',
					content_hash: 'b'.repeat(64)
				}
			],
			uncertainties: []
		}
	};
	let pending = false;
	const decisions: Array<Record<string, unknown>> = [];
	await page.route(
		new RegExp('/api/projects/project-1/ai/proposals(?:\\?.*)?$'),
		async (route) => {
			const url = new URL(route.request().url());
			expect(url.searchParams.get('target_record_id')).toBe('record-1');
			expect(url.searchParams.get('candidate_report_id')).toBe('report-1');
			await route.fulfill({
				json: { items: pending ? [aiProposal] : [], next_cursor: null }
			});
		}
	);
	await page.route(
		new RegExp('/api/projects/project-1/records/record-1/ai/deduplication$'),
		async (route) => {
			expect(route.request().method()).toBe('POST');
			expect(route.request().postDataJSON()).toEqual({ candidate_report_id: 'report-1' });
			await route.fulfill({
				status: 202,
				json: {
					id: 'dedupe-review-run',
					project_id: 'project-1',
					definition: 'duplicate_detection',
					subject: {},
					origin: { kind: 'reviewer_requested' },
					state: { kind: 'queued' },
					created_at: '2026-01-01T00:00:00Z'
				}
			});
		}
	);
	await page.route(
		'http://localhost:4173/api/projects/project-1/review-runs/dedupe-review-run',
		async (route) => {
			pending = true;
			await route.fulfill({
				json: {
					id: 'dedupe-review-run',
					project_id: 'project-1',
					definition: 'duplicate_detection',
					subject: {},
					origin: { kind: 'reviewer_requested' },
					state: { kind: 'completed', proposal_id: aiProposal.id },
					created_at: '2026-01-01T00:00:00Z',
					finished_at: '2026-01-01T00:00:01Z'
				}
			});
		}
	);
	await page.route(
		new RegExp('/api/projects/project-1/ai/proposals/ai-dedupe-proposal-1/decision$'),
		async (route) => {
			expect(route.request().method()).toBe('POST');
			decisions.push(route.request().postDataJSON());
			pending = false;
			await route.fulfill({
				status: 200,
				json: { data: { ...aiProposal, status: 'accepted' } }
			});
		}
	);

	await page.goto('/projects/project-1/deduplication');
	const ai = page.getByTestId('ai-proposal-review');
	await ai.getByRole('button', { name: 'Request suggestion' }).click();
	await expect(ai.getByText('match', { exact: true })).toBeVisible();
	await expect(ai.getByText('Stable title and author signals agree.')).toBeVisible();
	await expect(ai.getByTestId('ai-dedupe-provenance')).toContainText('Source record');
	await expect(ai.getByTestId('ai-dedupe-provenance')).toContainText('Candidate report');
	await ai.getByRole('button', { name: 'Approve and apply' }).click();
	await expect(ai.getByText('No pending suggestion', { exact: true })).toBeVisible();
	await expect.poll(() => decisions.length).toBe(1);
	expect(decisions[0]).toMatchObject({ decision: 'accept' });
});
