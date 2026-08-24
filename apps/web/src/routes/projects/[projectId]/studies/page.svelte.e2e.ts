import { expect, test, type Page } from '@playwright/test';

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
