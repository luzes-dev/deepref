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
	let releaseSave: (() => void) | undefined;
	let patchBody: unknown;
	const saveGate = new Promise<void>((resolve) => {
		releaseSave = resolve;
	});
	await routeSettings(page, async (route) => {
		patchBody = route.request().postDataJSON();
		await saveGate;
		await route.fulfill({ json: { ...settings, default_max_depth: 3 } });
	});

	await page.goto('/settings');
	await expect(page.getByLabel('Default max depth')).toHaveValue('2');

	await page.getByLabel('Default max depth').fill('-1');
	await expect(
		page.getByText('Default max depth must be an integer of at least 0.')
	).toBeVisible();
	await expect(page.getByTestId('save-settings')).toBeDisabled();

	await page.getByLabel('Default max depth').fill('3');
	await expect(page.getByTestId('settings-save-status')).toHaveText(/Unsaved changes/);
	await expect(page.getByTestId('save-settings')).toBeEnabled();

	await page.getByTestId('save-settings').click();
	await expect(page.getByTestId('save-settings')).toContainText('Saving settings');
	await expect(page.getByTestId('save-settings')).toBeDisabled();
	await expect(page.getByTestId('settings-save-status')).toHaveText(/Saving changes/);

	releaseSave?.();
	await expect(page.getByTestId('settings-save-status')).toHaveText(/Changes saved/);
	await expect(page.getByTestId('save-settings')).toBeDisabled();
	expect(patchBody).toEqual({
		crossref_mailto: 'research@example.org',
		default_max_depth: 3,
		max_concurrency: 8,
		rate_limit_per_second: 1,
		retry_attempts: 5
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
	await page.getByTestId('save-settings').click();

	await expect(page.getByTestId('settings-save-error')).toContainText(
		'Settings service is unavailable'
	);
	await expect(page.getByTestId('settings-save-status')).toHaveText(/Save failed/);
	await expect(page.getByLabel('Retry attempts')).toHaveValue('6');
	await expect(page.getByTestId('save-settings')).toBeEnabled();
});

test('renders an actionable load error when the settings API is unavailable', async ({ page }) => {
	await page.route(settingsUrl(), async (route) => {
		await route.fulfill({ status: 500, json: { message: 'Settings read failed' } });
	});

	await page.goto('/settings');
	await expect(page.getByTestId('settings-load-error')).toContainText('Settings read failed');
	await expect(page.getByRole('button', { name: 'Try again' })).toBeVisible();
});
