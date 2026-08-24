import { expect, test, type Page } from '@playwright/test';

const project = {
	id: 'project-1',
	name: 'Protocol project',
	description: 'A mocked project',
	default_max_depth: 2,
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z'
};

function noPublishedAt(): string | null {
	return null;
}

const dependencies = {
	postgresql: { state: 'available', lag: null, backlog: null, oldest_age_seconds: null },
	worker: { state: 'available', lag: 0, backlog: 0, oldest_age_seconds: null }
};

const draftProtocol = {
	id: 'protocol-draft',
	project_id: project.id,
	version: 1,
	name: 'Sleep review protocol',
	status: 'draft',
	framework_kind: 'pico',
	framework_fields: {
		population: 'Adults',
		intervention: 'Exercise',
		comparator: 'Usual care',
		outcome: 'Sleep quality'
	},
	objective: 'Assess the relationship between exercise and sleep.',
	question: 'Does exercise improve sleep quality in adults?',
	criteria: [
		{
			id: 'criterion-1',
			kind: 'inclusion',
			stage: 'both',
			dimension: 'population',
			label: 'Adult population',
			description: 'Participants are adults.',
			ordinal: 0
		}
	],
	revision: 1,
	amendment_of: null,
	published_at: noPublishedAt(),
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z'
};

type MockProtocol = Omit<typeof draftProtocol, 'published_at'> & {
	published_at: string | null;
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

test('deep-links, refreshes, publishes, and amends a protocol version', async ({ page }) => {
	let protocol: MockProtocol | undefined;
	const saves: Array<Record<string, unknown>> = [];

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
	await page.route(
		'http://localhost:4173/api/projects/project-1/review/protocol',
		async (route) => {
			if (route.request().method() === 'GET') {
				if (!protocol) {
					await route.fulfill({ status: 404, json: { message: 'protocol not found' } });
					return;
				}
				await route.fulfill({ json: protocol });
				return;
			}

			const body = route.request().postDataJSON();
			saves.push(body);
			protocol = {
				...draftProtocol,
				...body,
				id: 'protocol-draft',
				status: 'draft',
				framework_kind: body.framework.kind,
				framework_fields: body.framework.fields,
				criteria: body.criteria.map(
					(criterion: Record<string, unknown>, ordinal: number) => ({
						...criterion,
						id: criterion.id ?? `criterion-${ordinal + 1}`,
						ordinal
					})
				),
				revision: Number(body.expected_revision) + 1,
				amendment_of: body.protocol_version_id ?? null
			};
			await route.fulfill({ json: protocol });
		}
	);
	await page.route(
		'http://localhost:4173/api/projects/project-1/review/protocol/publish',
		async (route) => {
			expect(route.request().method()).toBe('POST');
			if (!protocol) throw new Error('publish called before save');
			protocol = { ...protocol, status: 'published', published_at: '2026-01-02T00:00:00Z' };
			await route.fulfill({ json: protocol });
		}
	);

	await page.goto('/projects/project-1/protocol');
	await expect(page).toHaveURL(/\/projects\/project-1\/protocol$/);
	await page.reload();
	await expect(page.getByRole('heading', { name: 'Review protocol' })).toBeVisible();

	await page.getByLabel('Name').fill(draftProtocol.name);
	await page.getByLabel('Objective').fill(draftProtocol.objective);
	await page.getByLabel('Question').fill(draftProtocol.question);
	await page.getByLabel('Population').fill('Adults');
	await page.getByLabel('Intervention').fill('Exercise');
	await page.getByLabel('Comparator').fill('Usual care');
	await page.getByLabel('Outcome').fill('Sleep quality');
	await page.getByRole('button', { name: 'Add criterion' }).click();
	await page.getByLabel('Label').last().fill('Adult population');
	await page.getByLabel('Description').last().fill('Participants are adults.');
	await page.getByRole('button', { name: 'Save draft' }).click();
	await expect(page.getByText('v1')).toBeVisible();
	await page.getByRole('button', { name: 'Publish version' }).click();
	await expect(page.getByText('published', { exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Amend published version' }).first().click();
	await page.getByLabel('Name').fill('Amended sleep review protocol');
	await page.getByRole('button', { name: 'Save draft' }).click();
	await expect.poll(() => saves.length).toBe(2);
	const amendment = saves[1];
	expect(amendment).toMatchObject({
		protocol_version_id: 'protocol-draft',
		expected_revision: 1
	});
});

test('preserves PICO values across custom framework switching and validates duplicate keys', async ({
	page
}) => {
	await mockProjectShell(page);
	await page.route(
		'http://localhost:4173/api/projects/project-1/review/protocol',
		async (route) => {
			if (route.request().method() === 'GET') {
				await route.fulfill({ status: 404, json: { message: 'protocol not found' } });
				return;
			}
			throw new Error('The duplicate-key scenario should not submit.');
		}
	);

	await page.goto('/projects/project-1/protocol');
	await expect(page.getByRole('heading', { name: 'Review protocol' })).toBeVisible();
	await page.getByLabel('Name').fill('PICO switching protocol');
	await page.getByLabel('Objective').fill('Test framework switching.');
	await page.getByLabel('Question').fill('Are framework values preserved?');
	await page.getByLabel('Population').fill('Adults');
	await page.getByLabel('Intervention').fill('Exercise');
	await page.getByLabel('Comparator').fill('Usual care');
	await page.getByLabel('Outcome').fill('Sleep quality');

	await page.getByRole('button', { name: 'Pico' }).click();
	await page.getByRole('option', { name: 'Custom' }).click();
	const customNames = page.getByPlaceholder('Field name');
	const customDefinitions = page.getByPlaceholder('Definition');
	await expect(customNames).toHaveCount(4);
	await expect(customNames.nth(0)).toHaveValue('population');
	await expect(customDefinitions.nth(0)).toHaveValue('Adults');

	await customNames.nth(0).fill('scope');
	await customNames.nth(1).fill(' scope ');
	await expect(
		page.getByText('Custom framework field names must be unique: scope.')
	).toBeVisible();
	await expect(page.getByRole('button', { name: 'Save draft' })).toBeDisabled();

	await customNames.nth(1).fill('intervention');
	await page.getByRole('button', { name: 'Custom' }).click();
	await page.getByRole('option', { name: 'Pico', exact: true }).click();
	await expect(page.getByLabel('Population')).toHaveValue('Adults');
	await expect(page.getByLabel('Intervention')).toHaveValue('Exercise');
	await page.getByRole('button', { name: 'Pico' }).click();
	await page.getByRole('option', { name: 'Custom' }).click();
	await expect(customNames.nth(0)).toHaveValue('scope');
	await expect(customNames.nth(1)).toHaveValue('intervention');
});

test('reconciles a stale save conflict before saving the refreshed revision', async ({ page }) => {
	await mockProjectShell(page);
	let getCount = 0;
	let saveCount = 0;
	const savedBodies: Array<Record<string, unknown>> = [];
	const serverProtocol = {
		...draftProtocol,
		name: 'Authoritative server protocol',
		revision: 2
	};
	await page.route(
		'http://localhost:4173/api/projects/project-1/review/protocol',
		async (route) => {
			if (route.request().method() === 'GET') {
				getCount += 1;
				await route.fulfill({ json: getCount === 1 ? draftProtocol : serverProtocol });
				return;
			}
			saveCount += 1;
			const body = route.request().postDataJSON();
			savedBodies.push(body);
			if (saveCount === 1) {
				await route.fulfill({ status: 409, json: { message: 'revision conflict' } });
				return;
			}
			await route.fulfill({
				json: {
					...serverProtocol,
					name: body.name,
					revision: 3
				}
			});
		}
	);

	await page.goto('/projects/project-1/protocol');
	await expect(page.getByLabel('Name')).toHaveValue(draftProtocol.name);
	await page.getByLabel('Name').fill('Stale local edit');
	await page.getByRole('button', { name: 'Save draft' }).click();
	await expect(page.getByText('Protocol changed elsewhere')).toBeVisible();

	await page.locator('[data-slot="alert-action"]').click();
	await expect(page.getByLabel('Name')).toHaveValue('Authoritative server protocol');
	await page.getByLabel('Name').fill('Recovered and saved');
	await page.getByRole('button', { name: 'Save draft' }).click();
	await expect.poll(() => saveCount).toBe(2);
	expect(savedBodies[1]).toMatchObject({ expected_revision: 2 });
	await expect(page.getByLabel('Name')).toHaveValue('Recovered and saved');
});
