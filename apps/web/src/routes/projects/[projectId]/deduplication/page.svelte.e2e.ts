import { expect, test } from '@playwright/test';

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
