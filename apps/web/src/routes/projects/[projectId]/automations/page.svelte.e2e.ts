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

async function expectAutomationManager(page: Page): Promise<void> {
	await expect(page.getByTestId('automation-manager')).toBeVisible();
	await expect(page.getByRole('heading', { name: 'Automations', exact: true })).toBeVisible();
	await expect(page.getByTestId('automation-editor')).toHaveCount(0);
}

async function expectAutomationEditor(page: Page): Promise<void> {
	await expect(page.getByTestId('automation-editor')).toBeVisible();
	await expect(page.getByTestId('automation-manager')).toHaveCount(0);
	await expect(page.getByTestId('automation-editor-canvas')).toBeVisible();
}

test('manages definitions from the list and opens a full-page editor', async ({ page }) => {
	await mockProjectShell(page);
	const mocked = await mockAutomationEndpoints(page);

	await page.goto('/projects/project-1/automations');
	await expect(page).toHaveURL(/\/projects\/project-1\/automations$/);
	await expectAutomationManager(page);
	await expect(page.getByRole('link', { name: 'Automations' })).toHaveAttribute(
		'href',
		'/projects/project-1/automations'
	);
	await expect(page.getByTestId('automation-definition-card-definition-1')).toContainText(
		'Event maintenance'
	);
	await expect(page.getByTestId('automation-definition-card-definition-1')).toContainText(
		'Active'
	);
	await expect(page.getByTestId('automation-definition-card-definition-2')).toContainText(
		'Manual maintenance'
	);
	await expect(page.getByTestId('automation-unsupported-definitions')).toContainText(
		'Future recipe'
	);

	await page.getByTestId('automation-edit-definition-definition-2').click();
	await expectAutomationEditor(page);
	await expect(
		page.getByRole('heading', { name: 'Manual maintenance', exact: true })
	).toBeVisible();
	await expect(page.locator('[data-workflow-node]')).toHaveCount(2);
	await expect(
		page.locator('[data-workflow-node]').getByText('Manual', { exact: true })
	).toBeVisible();
	await expect(
		page.getByTestId('workflow-editor').getByText('Recompute project metrics', { exact: true })
	).toBeVisible();
	await expect(page.getByLabel('Name')).toHaveValue('Manual maintenance');
	await expect(page.getByLabel('Name')).toHaveAttribute('readonly', '');

	await page
		.getByTestId('automation-editor-inspector')
		.getByLabel('Status')
		.selectOption('paused');
	await page.getByTestId('automation-editor-save-settings').click();
	await expect(page.getByTestId('automation-editor-feedback')).toContainText(
		'Automation settings saved.'
	);
	await expect(page.getByLabel('Name')).toHaveValue('Manual maintenance');
	await expect(mocked.state.lastConfigurePath).toBe(
		'/api/projects/project-1/automations/definitions/project_maintenance.v1'
	);
	await expect(mocked.state.lastConfigureBody).toMatchObject({
		name: 'Manual maintenance',
		status: 'paused'
	});

	page.once('dialog', async (dialog) => {
		await dialog.accept();
	});
	await page.getByTestId('automation-editor-back').click();
	await expectAutomationManager(page);
	await expect(page.getByTestId('automation-definition-card-definition-2')).toContainText(
		'Paused'
	);
});

test('creates an automation, edits its graph, reconnects nodes, and runs it', async ({ page }) => {
	await mockProjectShell(page);
	const mocked = await mockAutomationEndpoints(page);

	await page.goto('/projects/project-1/automations');
	await expectAutomationManager(page);
	await page.getByTestId('automation-add-definition').click();
	await expectAutomationEditor(page);

	await expect(page.getByLabel('Name')).not.toHaveAttribute('readonly');
	await page.getByLabel('Name').fill('Nightly maintenance');

	const nodes = page.locator('[data-workflow-node]');
	const edges = page.locator('[data-workflow-connection]');
	await expect(nodes).toHaveCount(2);
	await expect(edges).toHaveCount(1);
	const initialEdgeCount = await edges.count();

	const conditionGalleryItem = page.getByTestId('automation-editor-add-condition');
	await expect(conditionGalleryItem).toBeVisible();
	await conditionGalleryItem.click();
	await expect(nodes).toHaveCount(3);

	const actionNode = nodes.nth(1);
	const conditionNode = nodes.nth(2);
	const sourceHandle = actionNode.locator('[data-workflow-port="output"]').first();
	const targetHandle = conditionNode.locator('[data-workflow-port="input"]').first();
	await expect(sourceHandle).toBeVisible();
	await expect(targetHandle).toBeVisible();
	await sourceHandle.dragTo(targetHandle);
	await expect(edges).toHaveCount(initialEdgeCount + 1);

	await page.getByTestId('automation-editor-save-settings').click();
	await expect(page.getByTestId('automation-editor-feedback')).toContainText(
		'Automation created.'
	);
	await expect(page.getByLabel('Name')).toHaveValue('Nightly maintenance');
	await expect(mocked.state.definitions).toEqual(
		expect.arrayContaining([
			expect.objectContaining({ id: 'definition-3', name: 'Nightly maintenance' })
		])
	);

	mocked.state.expectedManualDefinitionId = 'definition-3';
	await expect(page.getByTestId('automation-editor-run')).toBeEnabled();
	await page.getByTestId('automation-editor-run').click();
	await expect(page.getByTestId('automation-editor-feedback')).toContainText(
		'Automation run queued.'
	);
	await expect(mocked.state.lastManualDefinitionId).toBe('definition-3');
	// Graph edits are browser-local. The server settings save intentionally does
	// not clear an unsaved graph, so accept the explicit discard before leaving
	// this newly-created editor.
	page.once('dialog', async (dialog) => {
		await dialog.accept();
	});
	await page.getByTestId('automation-editor-back').click();
	await expectAutomationManager(page);
	const runHistory = page.getByTestId('automation-run-history');
	await runHistory.locator('summary').click();
	await expect(page.getByTestId('automation-runs')).toBeVisible();
	await expect(page.getByTestId('automation-run')).toContainText('Completed');
	await page.getByTestId('automation-run').click();
	await expectAutomationEditor(page);
	await expect(page.getByTestId('automation-editor-server-status')).toContainText(/completed/i);
	await page.getByTestId('automation-editor-back').click();
	await expectAutomationManager(page);
});

test('persists the editor graph across reload and guards an unsaved return', async ({ page }) => {
	await mockProjectShell(page);
	await mockAutomationEndpoints(page, { definitions: [manualDefinition], runs: [run] });

	await page.goto('/projects/project-1/automations');
	await expectAutomationManager(page);
	await page.getByTestId('automation-edit-definition-definition-2').click();
	await expectAutomationEditor(page);

	const nodes = page.locator('[data-workflow-node]');
	await expect(nodes).toHaveCount(2);
	await page.getByTestId('automation-editor-add-condition').click();
	await expect(nodes).toHaveCount(3);
	await page.getByTestId('automation-editor-save-graph').click();
	await expect(page.getByTestId('automation-editor-local-status')).toContainText('saved');

	await page.reload();
	await expectAutomationManager(page);
	await page.getByTestId('automation-edit-definition-definition-2').click();
	await expectAutomationEditor(page);
	await expect(nodes).toHaveCount(3);

	await page.getByTestId('automation-editor-add-action').click();
	await expect(nodes).toHaveCount(4);
	await page.getByRole('button', { name: 'Select Action' }).last().click();
	await expect(page.getByRole('button', { name: 'Delete selected graph item' })).toBeEnabled();
	await page.getByRole('button', { name: 'Delete selected graph item' }).click();
	await expect(nodes).toHaveCount(3);
	await page.getByTestId('automation-editor-add-action').click();
	await expect(nodes).toHaveCount(4);

	let dialogType: string | null = null;
	let dialogMessage: string | null = null;
	page.once('dialog', async (dialog) => {
		dialogType = dialog.type();
		dialogMessage = dialog.message();
		await dialog.dismiss();
	});
	await page.getByTestId('automation-editor-back').click();
	await expect(page.getByTestId('automation-editor')).toBeVisible();
	await expect(dialogType).toBe('confirm');
	await expect(dialogMessage).toMatch(/unsaved|discard/i);

	page.once('dialog', async (dialog) => {
		await dialog.accept();
	});
	await page.getByTestId('automation-editor-back').click();
	await expectAutomationManager(page);
});

test('supports reversible graph gestures, typed connection guards, and safe remounts', async ({
	page
}) => {
	await mockProjectShell(page);
	await mockAutomationEndpoints(page, { definitions: [manualDefinition], runs: [run] });

	const consoleErrors: string[] = [];
	const pageErrors: string[] = [];
	page.on('console', (message) => {
		if (message.type() === 'error') consoleErrors.push(message.text());
	});
	page.on('pageerror', (error) => pageErrors.push(error.message));

	await page.goto('/projects/project-1/automations');
	await expectAutomationManager(page);
	await page.getByTestId('automation-edit-definition-definition-2').click();
	await expectAutomationEditor(page);

	const nodes = page.locator('[data-workflow-node]');
	const edges = page.locator('[data-workflow-connection]');
	await expect(nodes).toHaveCount(2);
	await expect(edges).toHaveCount(1);

	// Adding a node selects it after the Rete reconciliation completes, which
	// enables the inspector actions for keyboard and pointer users.
	await page.getByTestId('automation-editor-add-condition').click();
	await expect(nodes).toHaveCount(3);
	await expect(page.getByRole('button', { name: 'Delete selected graph item' })).toBeEnabled();
	await page.getByRole('button', { name: 'Fit to view', exact: true }).click();

	// A canvas control must not bubble its pointer gesture into node dragging.
	const conditionNode = nodes.nth(2);
	const conditionBefore = await conditionNode.boundingBox();
	const canvasExpression = page
		.getByTestId('workflow-editor')
		.getByRole('textbox', { name: 'expression' });
	await canvasExpression.fill('confidence >= 0.9');
	await expect.poll(async () => await canvasExpression.inputValue()).toBe('confidence >= 0.9');
	const conditionAfter = await conditionNode.boundingBox();
	if (!conditionBefore || !conditionAfter) throw new Error('Condition bounds are missing');
	expect(Math.abs(conditionAfter.x - conditionBefore.x)).toBeLessThan(2);
	expect(Math.abs(conditionAfter.y - conditionBefore.y)).toBeLessThan(2);

	// Drag the node header and confirm the persisted canvas position changes.
	const actionNode = nodes.nth(1);
	const actionHeader = actionNode.locator('header');
	const actionBefore = await actionNode.boundingBox();
	const actionHeaderBox = await actionHeader.boundingBox();
	if (!actionBefore || !actionHeaderBox) throw new Error('Action bounds are missing');
	await page.mouse.move(
		actionHeaderBox.x + actionHeaderBox.width / 2,
		actionHeaderBox.y + actionHeaderBox.height / 2
	);
	await page.mouse.down();
	await page.mouse.move(
		actionHeaderBox.x + actionHeaderBox.width / 2 + 70,
		actionHeaderBox.y + actionHeaderBox.height / 2 + 32,
		{ steps: 5 }
	);
	await page.mouse.up();
	await expect
		.poll(async () => {
			const current = await actionNode.boundingBox();
			return current ? current.x - actionBefore.x : 0;
		})
		.toBeGreaterThan(20);

	// Drag empty space to pan without changing the graph node count.
	const canvas = page.locator('[data-workflow-area]');
	const canvasBox = await canvas.boundingBox();
	if (!canvasBox) throw new Error('Workflow canvas bounds are missing');
	const viewport = page.locator('[data-workflow-viewport]');
	const viewportBefore = await viewport.evaluate(
		(element) => getComputedStyle(element).transform
	);
	await page.mouse.move(canvasBox.x + 18, canvasBox.y + 18);
	await page.mouse.down();
	await page.mouse.move(canvasBox.x + 74, canvasBox.y + 58, { steps: 5 });
	await page.mouse.up();
	await expect
		.poll(async () => viewport.evaluate((element) => getComputedStyle(element).transform))
		.not.toBe(viewportBefore);
	await expect(nodes).toHaveCount(3);

	// Trigger output -> condition input is rejected by the registry's data types.
	const sourceSelect = page.getByLabel('Output', { exact: true });
	const targetSelect = page.getByLabel('Input', { exact: true });
	const sourceChoices = await sourceSelect
		.locator('option')
		.evaluateAll<{ value: string; label: string }[], void, HTMLOptionElement>((options) =>
			options.map((option) => ({
				value: option.value,
				label: option.textContent ?? ''
			}))
		);
	const targetChoices = await targetSelect
		.locator('option')
		.evaluateAll<{ value: string; label: string }[], void, HTMLOptionElement>((options) =>
			options.map((option) => ({
				value: option.value,
				label: option.textContent ?? ''
			}))
		);
	const invalidSource = sourceChoices.find(
		(option) => option.label.includes('Trigger') && option.label.includes('automation.event')
	);
	const conditionInput = targetChoices.find(
		(option) => option.label.includes('Condition') && option.label.includes('automation.result')
	);
	if (!invalidSource || !conditionInput) throw new Error('Typed connection choices are missing');
	await sourceSelect.selectOption(invalidSource.value);
	await targetSelect.selectOption(conditionInput.value);
	await page.getByRole('button', { name: 'Connect ports', exact: true }).click();
	await expect(page.getByTestId('automation-editor-errors')).toContainText(/compatible|type/i);
	await expect(edges).toHaveCount(1);

	// Duplicate, undo, redo, and delete all use the canonical command history.
	await page.getByRole('button', { name: 'Select Condition' }).click();
	await page.getByRole('button', { name: 'Duplicate', exact: true }).click();
	await expect(nodes).toHaveCount(4);
	await page.getByTestId('automation-editor-undo').click();
	await expect(nodes).toHaveCount(3);
	await page.getByTestId('automation-editor-redo').click();
	await expect(nodes).toHaveCount(4);
	await expect(page.getByRole('button', { name: 'Delete selected graph item' })).toBeEnabled();
	await page.getByRole('button', { name: 'Delete selected graph item' }).click();
	await expect(nodes).toHaveCount(3);

	await page.getByTestId('automation-editor-save-graph').click();
	await expect(page.getByTestId('automation-editor-local-status')).toContainText('saved');

	// Close and reopen the same definition to exercise adapter teardown/remount.
	await page.getByTestId('automation-editor-back').click();
	await expectAutomationManager(page);
	await page.getByTestId('automation-edit-definition-definition-2').click();
	await expectAutomationEditor(page);
	await expect(nodes).toHaveCount(3);
	await expect(page.locator('[data-workflow-viewport]')).toHaveCount(1);

	await page.reload();
	await expectAutomationManager(page);
	await page.getByTestId('automation-edit-definition-definition-2').click();
	await expectAutomationEditor(page);
	await expect(nodes).toHaveCount(3);
	await expect(page.locator('[data-workflow-viewport]')).toHaveCount(1);

	expect(consoleErrors).toEqual([]);
	expect(pageErrors).toEqual([]);
});

test('protects malformed browser drafts from UI overwrite', async ({ page }) => {
	await mockProjectShell(page);
	await mockAutomationEndpoints(page, { definitions: [manualDefinition], runs: [run] });

	await page.goto('/projects/project-1/automations');
	await expectAutomationManager(page);
	await page.evaluate(() => {
		localStorage.setItem('deepref:automation-graph:project-1:definition-2', '{');
	});
	await page.getByTestId('automation-edit-definition-definition-2').click();
	await expectAutomationEditor(page);
	await expect(page.getByTestId('automation-editor-notice')).toContainText(/left untouched/i);
	await expect(page.getByTestId('automation-editor-save-graph')).toBeDisabled();
	await expect(
		await page.evaluate(() =>
			localStorage.getItem('deepref:automation-graph:project-1:definition-2')
		)
	).toBe('{');
});

test('shows the empty state and keeps manual execution disabled until configured', async ({
	page
}) => {
	await mockProjectShell(page);
	await mockAutomationEndpoints(page, { definitions: [], runs: [] });

	await page.goto('/projects/project-1/automations');
	await expectAutomationManager(page);
	await expect(page.getByTestId('automation-definitions-empty')).toBeVisible();
	await page.getByTestId('automation-run-history').locator('summary').click();
	await expect(page.getByTestId('automation-runs-empty')).toBeVisible();
	await page.getByTestId('automation-add-definition').click();
	await expectAutomationEditor(page);
	await expect(page.getByTestId('automation-editor-run')).toBeDisabled();
});

test('reports read failures and retries automation data', async ({ page }) => {
	await mockProjectShell(page);
	const mocked = await mockAutomationEndpoints(page, { failReads: true });

	await page.goto('/projects/project-1/automations');
	await expect(page.getByTestId('automation-query-error')).toBeVisible();

	mocked.state.failReads = false;
	await page.getByTestId('automation-query-error').getByRole('button', { name: 'Retry' }).click();
	await expectAutomationManager(page);
	await expect(page.getByTestId('automation-definition-card-definition-1')).toBeVisible();
	await page.getByTestId('automation-run-history').locator('summary').click();
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
	await expect(page.getByTestId('automation-list-loading')).toBeVisible();
	await expect(page.getByTestId('automation-page')).toHaveAttribute(
		'data-automation-state',
		'ready'
	);
	await expectAutomationManager(page);
	await page.getByTestId('automation-run-history').locator('summary').click();
	await expect(page.getByTestId('automation-run')).toContainText('Running');
	await page.getByTestId('automation-edit-definition-definition-2').click();
	await expectAutomationEditor(page);
	await expect(page.getByTestId('automation-editor-canvas')).toBeVisible();
	await page
		.getByTestId('automation-editor-inspector')
		.getByLabel('Status')
		.selectOption('paused');
	await page.getByTestId('automation-editor-save-settings').click();
	await expect(page.getByTestId('automation-editor-feedback')).toContainText(
		'Automation settings saved.'
	);

	await page.getByTestId('automation-editor-add-condition').click();
	await expect(page.locator('[data-workflow-node]')).toHaveCount(3);
	await page.getByTestId('automation-editor-save-graph').click();
	await page.getByTestId('automation-editor-back').click();
	await expectAutomationManager(page);

	const overflow = await page.evaluate(
		() => document.documentElement.scrollWidth > document.documentElement.clientWidth
	);
	expect(overflow).toBe(false);
});
