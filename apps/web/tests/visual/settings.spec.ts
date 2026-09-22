import type { Page } from '@playwright/test';
import { expect, test, captureViewport, isMobileViewport, settleVisualPage } from './fixtures';
import { runSeriousCriticalAxe } from './axe';

async function openSettingsOverlay(page: Page) {
	if (await isMobileViewport(page)) {
		await page.getByRole('button', { name: 'Open navigation' }).click();
		const sheet = page.getByTestId('mobile-navigation-sheet');
		await expect(sheet).toBeVisible();
		await sheet.getByRole('link', { name: 'Settings', exact: true }).click();
	} else {
		await page
			.getByTestId('project-sidebar')
			.getByRole('link', { name: 'Settings', exact: true })
			.click();
	}

	await expect(page).toHaveURL(/\/settings$/);
	const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
	await expect(dialog).toBeVisible();
	await settleVisualPage(page);
	return dialog;
}

test.describe('DeepRef settings pilot', () => {
	test('is ready at the review viewport without horizontal overflow', async ({ page }) => {
		await page.goto('/settings');
		await expect(page).toHaveURL(/\/settings$/);
		await expect(page.getByRole('heading', { name: 'Settings', exact: true })).toBeVisible();
		await expect(page.getByText('Ingestion defaults', { exact: true })).toBeVisible();
		await expect(page.getByLabel('Crossref mailto')).toHaveValue('research@example.org');
		await expect(page.getByRole('button', { name: 'Appearance', exact: true })).toBeVisible();

		const dimensions = await page.evaluate(() => ({
			bodyScrollWidth: document.body.scrollWidth,
			documentScrollWidth: document.documentElement.scrollWidth,
			viewportWidth: window.innerWidth
		}));
		expect(dimensions.bodyScrollWidth).toBeLessThanOrEqual(dimensions.viewportWidth);
		expect(dimensions.documentScrollWidth).toBeLessThanOrEqual(dimensions.viewportWidth);
		await captureViewport(page, 'settings.png');
	});

	test('has no serious or critical axe violations', async ({ page }) => {
		await page.goto('/settings');
		await expect(page.getByRole('heading', { name: 'Settings', exact: true })).toBeVisible();
		expect(await runSeriousCriticalAxe(page)).toEqual([]);
	});

	test('renders the Settings modal over the workspace', async ({ page }) => {
		const dialog = await openSettingsOverlay(page);
		await expect(dialog.getByRole('heading', { name: 'Ingestion', exact: true })).toBeVisible();
		await expect(page.getByTestId('overview-page')).toBeVisible();

		const violations = await runSeriousCriticalAxe(page);
		expect(violations, JSON.stringify(violations, null, 2)).toEqual([]);
		await captureViewport(page, 'settings-modal.png');
	});
});
