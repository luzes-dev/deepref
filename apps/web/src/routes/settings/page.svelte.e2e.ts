import { expect, test, type Page, type Route } from '@playwright/test';

const settings = {
	crossref_mailto: 'research@example.org',
	default_max_depth: 2,
	max_concurrency: 8,
	rate_limit_per_second: 1,
	retry_attempts: 5,
	metadata_provider: 'crossref',
	citation_provider: 'crossref'
};

test.beforeEach(async ({ page }) => {
	await page.route(
		/http:\/\/localhost:4173\/api\/notifications(?:\/unread-count)?$/,
		async (route) => {
			if (route.request().url().includes('unread-count')) {
				await route.fulfill({ json: { count: 0, latest_revision: 0 } });
			} else {
				await route.fulfill({ json: { items: [], next_cursor: null } });
			}
		}
	);
});

function settingsUrl(): RegExp {
	return /http:\/\/localhost:4173\/api\/settings$/;
}

async function routeSettings(
	page: Page,
	handlePatch: (route: Route) => Promise<void> = async (route) => {
		await route.fulfill({ json: settings });
	}
): Promise<void> {
	await page.route(settingsUrl(), async (route) => {
		if (route.request().method() === 'PATCH') {
			await handlePatch(route);
			return;
		}
		await route.fulfill({ json: settings });
	});
}

test('shows an explicit loading state while settings are being fetched', async ({ page }) => {
	let release: (() => void) | undefined;
	const responseGate = new Promise<void>((resolve) => {
		release = resolve;
	});
	await page.route(settingsUrl(), async (route) => {
		await responseGate;
		await route.fulfill({ json: settings });
	});

	await page.goto('/settings');
	await expect(page.getByTestId('settings-loading')).toBeVisible();

	release?.();
	await page.waitForLoadState('networkidle');
	await expect(page.getByText('Ingestion defaults', { exact: true })).toBeVisible();
	await expect(page.getByTestId('settings-save-status')).toHaveText(/Ready to edit/);
});

test('validates drafts, shows pending/saved states, and preserves the PATCH contract', async ({
	page
}) => {
	let patchBodies: unknown[] = [];
	await routeSettings(page, async (route) => {
		const body = route.request().postDataJSON();
		patchBodies = [...patchBodies, body];
		await route.fulfill({ json: { ...settings, ...(body as object) } });
	});

	await page.goto('/settings');
	await expect(page.getByLabel('Default max depth')).toHaveValue('2');
	await expect(page.getByRole('button', { name: /Save settings/i })).toHaveCount(0);
	await page.waitForTimeout(600);
	expect(patchBodies).toEqual([]);

	const depth = page.getByLabel('Default max depth');
	const depthField = depth.locator('xpath=..');
	const increase = depthField.getByRole('button', { name: 'Increase', exact: true });
	const decrease = depthField.getByRole('button', { name: 'Decrease', exact: true });
	await expect(increase).toBeVisible();
	await expect(decrease).toBeVisible();

	await increase.focus();
	await expect(increase).toBeFocused();
	await page.keyboard.press('Enter');
	await expect(depth).toHaveValue('3');
	await decrease.click();
	await expect(depth).toHaveValue('2');

	// All four edits happen inside one debounce window. The API receives only the
	// final value, including the keyboard and pointer interactions above.
	await increase.click();
	await increase.click();
	await expect(depth).toHaveValue('4');
	await expect.poll(() => patchBodies.length).toBe(1);
	await expect(page.getByTestId('settings-save-status')).toHaveText(/Changes saved/);

	expect(patchBodies[0]).toEqual({
		crossref_mailto: 'research@example.org',
		default_max_depth: 4,
		max_concurrency: 8,
		rate_limit_per_second: 1,
		retry_attempts: 5
	});
});

test('shows field validation errors and does not autosave an invalid draft', async ({ page }) => {
	const patchBodies: unknown[] = [];
	await routeSettings(page, async (route) => {
		patchBodies.push(route.request().postDataJSON());
		await route.fulfill({ json: settings });
	});

	await page.goto('/settings');
	await page.getByLabel('Default max depth').fill('1.5');
	await expect(
		page.getByText('Default max depth must be an integer of at least 0.')
	).toBeVisible();
	await expect(page.getByLabel('Default max depth')).toHaveAttribute('aria-invalid', 'true');
	await page.getByLabel('Default max depth').fill('');
	await expect(
		page.getByText('Default max depth must be an integer of at least 0.')
	).toBeVisible();
	await page.getByLabel('Crossref mailto').fill('');
	await expect(page.getByText('Crossref mailto is required.')).toBeVisible();

	await page.waitForTimeout(700);
	expect(patchBodies).toEqual([]);
});

test('keeps the latest edits and queues an autosave while the first request is pending', async ({
	page
}) => {
	const patchBodies: Record<string, unknown>[] = [];
	let releaseFirstSave: (() => void) | undefined;
	const firstSave = new Promise<void>((resolve) => {
		releaseFirstSave = resolve;
	});
	await routeSettings(page, async (route) => {
		const body = route.request().postDataJSON() as Record<string, unknown>;
		patchBodies.push(body);
		if (patchBodies.length === 1) await firstSave;
		await route.fulfill({ json: { ...settings, ...body } });
	});

	await page.goto('/settings');
	await page.getByLabel('Default max depth').fill('3');
	await expect.poll(() => patchBodies.length).toBe(1);
	await expect(page.getByTestId('settings-save-status')).toHaveText(/Saving changes/);

	const mailto = page.getByLabel('Crossref mailto');
	await expect(mailto).toBeEnabled();
	await mailto.fill('rapid@example.org');

	releaseFirstSave?.();
	await expect.poll(() => patchBodies.length).toBe(2);
	await expect(page.getByTestId('settings-save-status')).toHaveText(/Changes saved/);
	await expect(mailto).toHaveValue('rapid@example.org');
	await expect(page.getByLabel('Default max depth')).toHaveValue('3');
	expect(patchBodies[0]).toMatchObject({ default_max_depth: 3 });
	expect(patchBodies[1]).toMatchObject({
		crossref_mailto: 'rapid@example.org',
		default_max_depth: 3
	});
});

test('keeps the draft and explains an API save error', async ({ page }) => {
	await routeSettings(page, async (route) => {
		await route.fulfill({
			status: 500,
			json: { message: 'Settings service is unavailable', code: 'settings_unavailable' }
		});
	});

	await page.goto('/settings');
	await page.getByLabel('Retry attempts').fill('6');

	await expect(page.locator('[data-sonner-toast]')).toContainText('Could not save settings');
	await expect(page.locator('[data-sonner-toast]')).toContainText(
		'Settings service is unavailable'
	);
	await expect(page.getByTestId('settings-save-status')).toHaveText(/Save failed/);
	await expect(page.getByLabel('Retry attempts')).toHaveValue('6');
	await expect(page.getByLabel('Retry attempts')).toBeEnabled();
});

test('selects a theme from the Appearance settings control', async ({ page }) => {
	await routeSettings(page);

	await page.goto('/settings');
	await page.getByRole('button', { name: 'Appearance', exact: true }).click();
	await expect(page.getByRole('heading', { name: 'Appearance', exact: true })).toBeVisible();

	const themeSelect = page.getByRole('button', { name: 'Theme', exact: true });
	await expect(themeSelect).toBeVisible();
	await themeSelect.click();
	await expect(page.getByRole('option', { name: 'Dark', exact: true })).toBeVisible();
	await page.getByRole('option', { name: 'Dark', exact: true }).click();
	await expect
		.poll(() => page.evaluate(() => document.documentElement.classList.contains('dark')))
		.toBe(true);
});

test('renders an actionable load error when the settings API is unavailable', async ({ page }) => {
	await page.route(settingsUrl(), async (route) => {
		await route.fulfill({ status: 500, json: { message: 'Settings read failed' } });
	});

	await page.goto('/settings');
	await expect(page.getByTestId('settings-load-error')).toContainText('Settings read failed');
	await expect(page.getByRole('button', { name: 'Try again' })).toBeVisible();
});
