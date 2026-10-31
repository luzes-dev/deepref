import { expect, test } from '../../tests/e2e-test';
import type { Page, Route } from '@playwright/test';

const settings = {
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

async function routeSettings(page: Page): Promise<string[]> {
	const methods: string[] = [];
	await page.route(settingsUrl(), async (route: Route) => {
		methods.push(route.request().method());
		await route.fulfill({ json: settings });
	});
	return methods;
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
	await expect(page.getByLabel('Default max depth')).toHaveText('2');
});

test('shows deployment-wide limits read-only without edit controls', async ({ page }) => {
	const methods = await routeSettings(page);

	await page.goto('/settings');
	await expect(page.getByLabel('Default max depth')).toHaveText('2');
	await expect(page.getByLabel('Max concurrency')).toHaveText('8');
	await expect(page.getByLabel('Rate limit per second')).toHaveText('1');
	await expect(page.getByLabel('Retry attempts')).toHaveText('5');
	await expect(page.getByTestId('settings-read-only-note')).toBeVisible();
	await expect(page.getByRole('spinbutton')).toHaveCount(0);
	await expect(page.getByTestId('settings-save-status')).toHaveCount(0);
	await page.waitForTimeout(600);
	expect(methods.filter((method) => method !== 'GET')).toEqual([]);
});

test('selects a theme from the Appearance settings control', async ({ page }) => {
	await routeSettings(page);

	await page.goto('/settings');
	await page.getByRole('button', { name: 'Appearance', exact: true }).click();

	const themeSelect = page.getByRole('combobox', { name: 'Theme', exact: true });
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
