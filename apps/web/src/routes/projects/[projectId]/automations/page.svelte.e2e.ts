import { expect, test, type Page } from '@playwright/test';

const api = 'http://localhost:4173/api';
const project = {
	id: 'project-1',
	name: 'Automation project',
	description: 'A mocked project',
	default_max_depth: 2,
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z'
};
const dependencies = {
	postgresql: { state: 'available', lag: null, backlog: null, oldest_age_seconds: null },
	worker: { state: 'available', lag: 0, backlog: 0, oldest_age_seconds: null }
};
const definition = {
	id: 'definition-1',
	project_id: 'project-1',
	name: 'Event maintenance',
	recipe: 'project_maintenance',
	version: 1,
	trigger: 'report_added',
	status: 'active',
	steps: [{ ordinal: 0, key: 'recompute_project_metrics', kind: 'deterministic_action' }],
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z'
};
const manualDefinition = {
	...definition,
	id: 'definition-2',
	name: 'Manual maintenance',
	trigger: 'manual'
};
const unsupportedDefinition = {
	...definition,
	id: 'definition-unsupported',
	name: 'Future recipe',
	recipe: 'future_recipe',
	version: 2
};
const run = {
	id: 'run-1',
	project_id: 'project-1',
	definition_id: 'definition-2',
	recipe: 'project_maintenance',
	version: 1,
	trigger: 'manual',
	trigger_reference: null,
	status: 'completed',
	created_at: '2026-01-02T03:04:00Z',
	started_at: '2026-01-02T03:04:02Z',
	finished_at: '2026-01-02T03:05:00Z',
	error: null,
	job: {
		id: 'job-1',
		status: 'completed',
		attempts: 1,
		max_attempts: 3,
		available_at: '2026-01-02T03:04:00Z',
		leased_until: null,
		last_error: null
	},
	steps: [
		{
			id: 'step-run-1',
			ordinal: 0,
			key: 'recompute_project_metrics',
			kind: 'deterministic_action',
			status: 'completed',
			attempts: 1,
			claimed_by: 'worker-1',
			started_at: '2026-01-02T03:04:02Z',
			finished_at: '2026-01-02T03:05:00Z',
			error: null
		}
	],
	usage: { input_tokens: 123, output_tokens: 456, cost_micros: 123456 }
};

type DefinitionFixture = typeof definition;
type RunFixture = typeof run;
type ConfigureBody = { name: string; trigger: string; status: string };
type MockState = {
	definitions: DefinitionFixture[];
	runs: RunFixture[];
	failReads: boolean;
	lastConfigurePath: string | null;
	lastConfigureBody: ConfigureBody | null;
	expectedManualDefinitionId: string | null;
	lastManualDefinitionId: string | null;
};

const runningRun: RunFixture = {
	...run,
	status: 'running',
	started_at: '2026-01-02T03:04:02Z',
	finished_at: '2026-01-02T03:04:30Z',
	job: { ...run.job, status: 'running' },
	steps: [{ ...run.steps[0], status: 'running', finished_at: '2026-01-02T03:04:30Z' }]
};

async function mockProjectShell(page: Page): Promise<void> {
	await page.route(`${api}/health/dependencies`, (route) =>
		route.fulfill({ json: dependencies })
	);
	await page.route(/\/api\/projects(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [project], next_cursor: null } })
	);
	await page.route(`${api}/projects/project-1`, (route) => route.fulfill({ json: project }));
	await page.route(/\/api\/projects\/project-1\/reports(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [], next_cursor: null } })
	);
	await page.route(/\/api\/ingestions(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [], next_cursor: null } })
	);
}

async function mockAutomationEndpoints(
	page: Page,
	options: {
		definitions?: DefinitionFixture[];
		runs?: RunFixture[];
		failReads?: boolean;
		delayReads?: boolean;
	} = {}
): Promise<{ state: MockState }> {
	const state: MockState = {
		definitions: options.definitions ?? [definition, manualDefinition, unsupportedDefinition],
		runs: options.runs ?? [run],
		failReads: options.failReads ?? false,
		lastConfigurePath: null,
		lastConfigureBody: null,
		expectedManualDefinitionId: null,
		lastManualDefinitionId: null
	};

	await page.route(`${api}/projects/project-1/automations/definitions`, async (route) => {
		if (route.request().method() !== 'GET')
			throw new Error('Unexpected automation definition method');
		if (state.failReads) {
			await route.fulfill({ status: 500, json: { message: 'definitions unavailable' } });
			return;
		}
		if (options.delayReads) await new Promise((resolve) => setTimeout(resolve, 750));
		await route.fulfill({ json: state.definitions });
	});
	await page.route(
		`${api}/projects/project-1/automations/definitions/project_maintenance.v1`,
		async (route) => {
			expect(route.request().method()).toBe('PUT');
			expect(route.request().headers()['x-actor-kind']).toBe('user');
			expect(route.request().headers()['x-actor-id']).toBe('local-user');
			const body = route.request().postDataJSON();
			expect(body).toMatchObject({
				name: expect.any(String),
				trigger: expect.any(String),
				status: expect.any(String)
			});
			state.lastConfigurePath = new URL(route.request().url()).pathname;
			state.lastConfigureBody = {
				name: body.name,
				trigger: body.trigger,
				status: body.status
			};
			const existing = state.definitions.find(
				(candidate) =>
					candidate.recipe === 'project_maintenance' && candidate.name === body.name
			);
			const updated = existing
				? { ...existing, trigger: body.trigger, status: body.status }
				: {
						...definition,
						id: 'definition-3',
						name: body.name,
						trigger: body.trigger,
						status: body.status
					};
			state.definitions = existing
				? state.definitions.map((candidate) =>
						candidate.id === existing.id ? updated : candidate
					)
				: [...state.definitions, updated];
			await route.fulfill({ json: updated });
		}
	);
	await page.route(/\/api\/projects\/project-1\/automations\/runs(?:\?.*)?$/, async (route) => {
		if (route.request().method() === 'GET') {
			if (state.failReads) {
				await route.fulfill({ status: 500, json: { message: 'runs unavailable' } });
				return;
			}
			if (options.delayReads) await new Promise((resolve) => setTimeout(resolve, 750));
			await route.fulfill({ json: state.runs });
			return;
		}

		expect(route.request().method()).toBe('POST');
		expect(route.request().headers()['x-actor-kind']).toBe('user');
		expect(route.request().headers()['x-actor-id']).toBe('local-user');
		expect(route.request().headers()['idempotency-key']).toMatch(
			/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i
		);
		const body = route.request().postDataJSON();
		expect(body).toEqual({ definition_id: state.expectedManualDefinitionId });
		state.lastManualDefinitionId = body.definition_id;
		const selected = state.definitions.find(
			(candidate) => candidate.id === state.lastManualDefinitionId
		);
		if (!selected) throw new Error('The selected fixture definition was not found');
		state.runs = [
			{
				...run,
				definition_id: selected.id,
				recipe: selected.recipe,
				version: selected.version,
				trigger: selected.trigger
			}
		];
		await route.fulfill({
			status: 201,
			json: { created: true, job_id: 'job-1', run_id: 'run-1' }
		});
	});
	await page.route(`${api}/projects/project-1/automations/runs/run-1`, async (route) => {
		expect(route.request().method()).toBe('GET');
		await route.fulfill({ json: state.runs[0] });
	});

	return { state };
}

test('selects, updates, creates, and runs the explicitly selected definition', async ({ page }) => {
	await mockProjectShell(page);
	const mocked = await mockAutomationEndpoints(page);

	await page.goto('/projects/project-1/automations');
	await expect(page).toHaveURL(/\/projects\/project-1\/automations$/);
	await expect(page.getByRole('heading', { name: 'Automation Center' })).toBeVisible();
	await expect(page.getByRole('link', { name: 'Automations' })).toHaveAttribute(
		'href',
		'/projects/project-1/automations'
	);
	await expect(page.getByTestId('automation-unsupported-definitions')).toContainText(
		'Future recipe'
	);

	await page.getByTestId('automation-definition-select').click();
	await expect(page.getByRole('option', { name: /Event maintenance/ })).toBeVisible();
	await expect(page.getByRole('option', { name: /Manual maintenance/ })).toBeVisible();
	await expect(page.getByRole('option', { name: /Future recipe/ })).toHaveCount(0);
	await page.getByRole('option', { name: /Manual maintenance/ }).click();
	await expect(page.getByLabel('Name')).toHaveValue('Manual maintenance');
	await expect(page.getByLabel('Name')).toHaveAttribute('readonly', '');

	await page.getByTestId('automation-status-paused').click();
	await page.getByTestId('automation-save-definition').click();
	await expect(page.getByTestId('automation-success')).toContainText(
		'Definition settings saved.'
	);
	await expect(page.getByLabel('Name')).toHaveValue('Manual maintenance');
	await expect(mocked.state.lastConfigurePath).toBe(
		'/api/projects/project-1/automations/definitions/project_maintenance.v1'
	);
	await expect(mocked.state.lastConfigureBody).toMatchObject({
		name: 'Manual maintenance',
		status: 'paused'
	});

	await page.getByTestId('automation-add-definition').click();
	await expect(page.getByLabel('Name')).not.toHaveAttribute('readonly');
	await page.getByLabel('Name').fill('Nightly maintenance');
	await page.getByTestId('automation-save-definition').click();
	await expect(page.getByTestId('automation-success')).toContainText(
		'Definition created and selected.'
	);
	await expect(page.getByLabel('Name')).toHaveValue('Nightly maintenance');
	await expect(mocked.state.definitions).toEqual(
		expect.arrayContaining([
			expect.objectContaining({ id: 'definition-3', name: 'Nightly maintenance' })
		])
	);

	mocked.state.expectedManualDefinitionId = 'definition-3';
	await expect(page.getByTestId('automation-run-manually')).toBeEnabled();
	await page.getByTestId('automation-run-manually').click();
	await expect(page.getByTestId('automation-success')).toContainText('Automation run queued.');
	await expect(mocked.state.lastManualDefinitionId).toBe('definition-3');
	await expect(page.getByTestId('automation-run')).toContainText('Completed');
	await expect(page.getByTestId('automation-run')).toContainText('123,456 micros');
	await expect(page.getByTestId('automation-run')).toContainText('Input tokens');
	await expect(page.getByTestId('automation-run')).toContainText('recompute_project_metrics');
	await expect(page.getByTestId('automation-run-details')).toContainText('Completed');
});

test('shows the empty state and keeps manual execution disabled until configured', async ({
	page
}) => {
	await mockProjectShell(page);
	await mockAutomationEndpoints(page, { definitions: [], runs: [] });

	await page.goto('/projects/project-1/automations');
	await expect(page.getByTestId('automation-runs-empty')).toBeVisible();
	await expect(page.getByTestId('automation-manual-state')).toContainText(
		'Add a definition above'
	);
	await expect(page.getByTestId('automation-run-manually')).toBeDisabled();
});

test('reports read failures and retries automation data', async ({ page }) => {
	await mockProjectShell(page);
	const mocked = await mockAutomationEndpoints(page, { failReads: true });

	await page.goto('/projects/project-1/automations');
	await expect(page.getByTestId('automation-query-error')).toBeVisible();

	mocked.state.failReads = false;
	await page.getByTestId('automation-query-error').getByRole('button', { name: 'Retry' }).click();
	await expect(page.getByRole('heading', { name: 'Recent runs' })).toBeVisible();
	await expect(page.getByTestId('automation-runs')).toBeVisible();
});

test('exposes loading, active-run, and completed-run presentation responsively', async ({
	page
}) => {
	await mockProjectShell(page);
	await mockAutomationEndpoints(page, {
		definitions: [manualDefinition],
		runs: [runningRun],
		delayReads: true
	});
	await page.emulateMedia({ colorScheme: 'dark', reducedMotion: 'reduce' });
	await page.setViewportSize({ width: 390, height: 844 });
	await page.goto('/projects/project-1/automations');
	await expect(page.getByTestId('automation-page')).toHaveAttribute(
		'data-automation-state',
		'loading'
	);
	await expect(page.getByTestId('automation-runs-loading')).toBeVisible();
	await expect(page.getByTestId('automation-page')).toHaveAttribute(
		'data-automation-state',
		'ready'
	);
	await expect(page.getByTestId('automation-run')).toContainText('Running');

	const overflow = await page.evaluate(
		() => document.documentElement.scrollWidth > document.documentElement.clientWidth
	);
	expect(overflow).toBe(false);
});
