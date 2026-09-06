import { test, expect, captureViewport, isMobileViewport } from './fixtures';
import { runSeriousCriticalAxe } from './axe';

test.describe('DeepRef workspace visual and accessibility harness', () => {
	test('renders the deterministic overview shell', async ({ page }) => {
		await expect(page).toHaveTitle(/DeepRef/i);
		await expect(
			page.getByRole('heading', { name: 'Evidence synthesis workspace' })
		).toBeVisible();
		await expect(
			page.getByRole('heading', { name: 'Recent ingestion activity' })
		).toBeVisible();
		await expect(page.getByRole('combobox', { name: 'Select project' })).toBeVisible();
		await expect(page).toHaveURL(/\/projects\/visual-project\/overview$/);

		const project = test
			.info()
			.project.name.replaceAll(/[^a-z0-9]+/gi, '-')
			.toLowerCase();
		await captureViewport(page, `${project}-overview.png`);
	});

	test('keeps evidence visible when dependency health is unavailable', async ({ page }) => {
		await page.route('**/api/health/dependencies', (route) =>
			route.fulfill({ status: 503, json: { detail: 'health unavailable' } })
		);
		await page.reload();
		await expect(page.getByTestId('overview-dependency-warning')).toBeVisible();
		await expect(page.getByTestId('overview-populated')).toBeVisible();
		await expect(
			page.getByRole('heading', { name: 'Recent ingestion activity' })
		).toBeVisible();
	});

	test('keeps project selection keyboard-safe and Escape-closable', async ({ page }) => {
		const trigger = page.getByRole('combobox', { name: 'Select project' });
		await trigger.click();
		await expect(page.getByPlaceholder('Search projects...')).toBeVisible();
		await page.keyboard.press('Escape');
		await expect(page.getByPlaceholder('Search projects...')).toBeHidden();
		await expect(trigger).toBeFocused();

		await page.keyboard.press('Tab');
		const focused = page.locator(':focus');
		await expect(focused).toBeVisible();
	});

	test('retains usable content at 200 percent layout zoom', async ({ page }) => {
		await page.evaluate(() => {
			document.documentElement.style.zoom = '2';
		});
		await expect(
			page.getByRole('heading', { name: 'Evidence synthesis workspace' })
		).toBeVisible();
		await expect(page.getByRole('combobox', { name: 'Select project' })).toBeVisible();
		await expect(page.locator('body')).toHaveCSS('overflow-x', /auto|visible/);
	});

	test('honours reduced motion and has no undersized touch targets', async ({ page }) => {
		await expect(
			await page.evaluate(() => matchMedia('(prefers-reduced-motion: reduce)').matches)
		).toBe(true);
		const undersizedVisibleControls = await page
			.locator('button, a, input, select, textarea, [role="button"]')
			.evaluateAll((elements) =>
				elements
					.filter((element) => {
						const rect = element.getBoundingClientRect();
						const style = getComputedStyle(element);
						return (
							style.display !== 'none' &&
							style.visibility !== 'hidden' &&
							rect.width > 0 &&
							rect.height > 0 &&
							(rect.width < 24 || rect.height < 24)
						);
					})
					.map((element) => ({
						tag: element.tagName.toLowerCase(),
						label:
							element.getAttribute('aria-label') ??
							element.textContent?.trim().slice(0, 60)
					}))
			);
		expect(undersizedVisibleControls).toEqual([]);
	});

	test('exposes table semantics for the desktop article collection', async ({ page }) => {
		if (await isMobileViewport(page))
			test.skip(true, 'Article cards replace the table below 768px.');
		await page.goto('/projects/visual-project/articles');
		await page.waitForLoadState('networkidle');
		await expect(page.getByRole('heading', { name: 'Articles' })).toBeVisible();
		const region = page.getByRole('region', { name: 'Project articles' });
		await expect(region).toBeVisible();
		await expect(page.getByRole('table')).toBeVisible();
		await expect(page.getByRole('columnheader').first()).toBeVisible();
		await expect(await page.getByRole('row').count()).toBeGreaterThan(1);
		await expect(await page.getByRole('cell').count()).toBeGreaterThan(1);
		const isScrollable = await region.evaluate(
			(element) => element.scrollWidth > element.clientWidth + 1
		);
		if (isScrollable) await expect(region).toHaveAttribute('tabindex', '0');
		else await expect(region).not.toHaveAttribute('tabindex');
	});

	test('has no serious or critical axe violations', async ({ page }) => {
		const violations = await runSeriousCriticalAxe(page);
		expect(violations, JSON.stringify(violations, null, 2)).toEqual([]);
	});

	test('exposes a working skip-navigation link', async ({ page }) => {
		const skipLink = page.getByRole('link', { name: 'Skip to content', exact: true });
		await expect(skipLink).toBeVisible();
		await skipLink.focus();
		await page.keyboard.press('Enter');
		await expect(page.locator('main, [role="main"]').first()).toBeFocused();
	});
});
