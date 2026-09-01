import { expect, test } from './fixtures';
import { runSeriousCriticalAxe } from './axe';

test.describe('DeepRef settings pilot', () => {
	test('is ready at the review viewport without horizontal overflow', async ({ page }) => {
		await page.goto('/settings');
		await expect(page).toHaveURL(/\/settings$/);
		await expect(page.getByRole('heading', { name: 'Settings', exact: true })).toBeVisible();
		await expect(page.getByText('Ingestion defaults', { exact: true })).toBeVisible();
		await expect(page.getByLabel('Crossref mailto')).toHaveValue('research@example.org');
		await expect(page.getByRole('button', { name: 'Toggle theme' })).toBeVisible();

		const dimensions = await page.evaluate(() => ({
			bodyScrollWidth: document.body.scrollWidth,
			documentScrollWidth: document.documentElement.scrollWidth,
			viewportWidth: window.innerWidth
		}));
		expect(dimensions.bodyScrollWidth).toBeLessThanOrEqual(dimensions.viewportWidth);
		expect(dimensions.documentScrollWidth).toBeLessThanOrEqual(dimensions.viewportWidth);
		await expect(page).toHaveScreenshot('settings.png');
	});

	test('has no serious or critical axe violations', async ({ page }) => {
		await page.goto('/settings');
		await expect(page.getByRole('heading', { name: 'Settings', exact: true })).toBeVisible();
		expect(await runSeriousCriticalAxe(page)).toEqual([]);
	});
});
