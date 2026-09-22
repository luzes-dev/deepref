import { expect, type Page } from '@playwright/test';
import { settleVisualPage, test } from './visual/fixtures';

const articlesUrl = '/projects/visual-project/articles?filter=review&sort=year';

const initialSettings = {
	crossref_mailto: 'research@example.org',
	default_max_depth: 2,
	max_concurrency: 8,
	rate_limit_per_second: 1,
	retry_attempts: 5,
	metadata_provider: 'crossref',
	citation_provider: 'crossref'
};

async function openSettingsModal(page: Page) {
	const trigger = page
		.getByTestId('project-sidebar')
		.getByRole('link', { name: 'Settings', exact: true });
	await trigger.focus();
	await trigger.click();

	await expect(page).toHaveURL(/\/settings$/);
	const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
	await expect(dialog).toBeVisible();
	return { dialog, trigger };
}

async function openMobileSettingsModal(page: Page) {
	await page.getByRole('button', { name: 'Open navigation' }).click();
	const sheet = page.getByTestId('mobile-navigation-sheet');
	await expect(sheet).toBeVisible();
	const trigger = sheet.getByRole('link', { name: 'Settings', exact: true });
	await trigger.focus();
	await trigger.click();

	await expect(page).toHaveURL(/\/settings$/);
	const dialog = page.getByRole('dialog', { name: 'Settings', exact: true });
	await expect(dialog).toBeVisible();
	return {
		dialog,
		trigger,
		mobileNavigationTrigger: page.getByRole('button', { name: 'Open navigation' })
	};
}

async function openArticles(page: Page): Promise<void> {
	await page.goto(articlesUrl);
	await expect(page).toHaveURL(/\/projects\/visual-project\/articles\?filter=review&sort=year$/);
	await expect(page.getByTestId('articles-page')).toBeVisible();
	await expect(page.getByRole('heading', { name: 'Articles', exact: true })).toBeVisible();
}

test('opens Settings shallowly while preserving the article page DOM and draft', async ({
	page
}) => {
	await page.setViewportSize({ width: 1280, height: 860 });
	await openArticles(page);

	const articleSearch = page.getByRole('textbox', { name: 'Search articles' });
	await articleSearch.fill('Citation');
	await expect(articleSearch).toHaveValue('Citation');
	await page.getByTestId('articles-page').evaluate((node) => {
		node.setAttribute('data-shallow-test-marker', 'mounted-before-settings');
	});

	const { dialog } = await openSettingsModal(page);
	await expect(dialog.getByRole('heading', { name: 'Ingestion', exact: true })).toBeVisible();
	await expect(page.getByTestId('articles-page')).toHaveAttribute(
		'data-shallow-test-marker',
		'mounted-before-settings'
	);
	await expect(articleSearch).toHaveValue('Citation');
});

test('opens Settings shallowly from the mobile navigation', async ({ page }) => {
	await page.setViewportSize({ width: 390, height: 844 });
	await openArticles(page);
	await expect(page.getByRole('button', { name: 'Open navigation' })).toBeVisible();

	await openMobileSettingsModal(page);
	await expect(page.getByTestId('articles-page')).toBeVisible();
	await expect(page.getByRole('dialog', { name: 'Settings', exact: true })).toBeVisible();
});

test('searches Settings content and keeps only matching sections', async ({ page }) => {
	await page.setViewportSize({ width: 1280, height: 860 });
	await openArticles(page);
	const { dialog } = await openSettingsModal(page);

	const search = dialog.getByRole('textbox', { name: 'Search settings' });
	await search.fill('citation depth');
	await expect(dialog.getByRole('button', { name: 'Ingestion', exact: true })).toBeVisible();
	await expect(dialog.getByRole('button', { name: 'Appearance', exact: true })).toHaveCount(0);
	await expect(
		dialog.getByRole('button', { name: 'Provider defaults', exact: true })
	).toHaveCount(0);
	await dialog.getByRole('button', { name: 'Ingestion', exact: true }).click();
	await expect(dialog.getByLabel('Default max depth')).toBeVisible();
});

test('supports browser back and forward for the Settings overlay', async ({ page }) => {
	await page.setViewportSize({ width: 1280, height: 860 });
	await openArticles(page);
	const originalUrl = page.url();

	await openSettingsModal(page);
	await page.goBack();
	await expect(page).toHaveURL(originalUrl);
	await expect(page.getByRole('dialog', { name: 'Settings', exact: true })).toHaveCount(0);
	await expect(page.getByTestId('articles-page')).toBeVisible();

	await page.goForward();
	await expect(page).toHaveURL(/\/settings$/);
	await expect(page.getByRole('dialog', { name: 'Settings', exact: true })).toBeVisible();
});

test('traps focus in the Settings dialog and restores it after Escape', async ({ page }) => {
	await page.setViewportSize({ width: 1280, height: 860 });
	await openArticles(page);
	const { dialog, trigger } = await openSettingsModal(page);

	await expect
		.poll(() =>
			page.evaluate(() => {
				const active = document.activeElement;
				return active instanceof HTMLElement && Boolean(active.closest('[role="dialog"]'));
			})
		)
		.toBe(true);

	const focusable = dialog.locator(
		'button:not([disabled]):not([tabindex="-1"]), a[href], input:not([disabled]):not([tabindex="-1"]), select:not([disabled]):not([tabindex="-1"]), textarea:not([disabled]):not([tabindex="-1"]), [tabindex]:not([tabindex="-1"]):not([disabled])'
	);
	const focusableCount = await focusable.count();
	expect(focusableCount).toBeGreaterThan(1);
	await focusable.nth(focusableCount - 1).focus();
	await page.keyboard.press('Tab');
	await expect(focusable.first()).toBeFocused();

	await page.keyboard.press('Escape');
	await expect(dialog).toHaveCount(0);
	await expect(page).toHaveURL(/\/projects\/visual-project\/articles\?filter=review&sort=year$/);
	await expect(trigger).toBeFocused();
});

test('closes the Settings overlay with its explicit close action', async ({ page }) => {
	await page.setViewportSize({ width: 1280, height: 860 });
	await openArticles(page);
	await openSettingsModal(page);

	await page.getByRole('button', { name: 'Close settings', exact: true }).click();
	await expect(page).toHaveURL(/\/projects\/visual-project\/articles\?filter=review&sort=year$/);
	await expect(page.getByRole('dialog', { name: 'Settings', exact: true })).toHaveCount(0);
	await expect(page.getByTestId('articles-page')).toBeVisible();
});

test('refreshing a shallow Settings URL renders the standalone page', async ({ page }) => {
	await page.setViewportSize({ width: 1280, height: 860 });
	await openArticles(page);
	await openSettingsModal(page);

	await page.reload();
	await expect(page).toHaveURL(/\/settings$/);
	await expect(page.getByTestId('settings-page')).toBeVisible();
	await expect(page.getByRole('dialog', { name: 'Settings', exact: true })).toHaveCount(0);
	await expect(page.getByRole('heading', { name: 'Settings', exact: true })).toBeVisible();
});

test('restores focus to the mobile navigation trigger after closing Settings', async ({ page }) => {
	await page.setViewportSize({ width: 390, height: 844 });
	await openArticles(page);
	const { dialog, mobileNavigationTrigger } = await openMobileSettingsModal(page);

	await dialog.getByRole('button', { name: 'Close settings', exact: true }).click();
	await expect(dialog).toHaveCount(0);
	await expect(page).toHaveURL(/\/projects\/visual-project\/articles\?filter=review&sort=year$/);
	await expect(mobileNavigationTrigger).toBeFocused();
});

test('direct, refreshed, and new-tab Settings URLs render the full page', async ({ page }) => {
	await page.goto('/settings');
	await expect(page.getByTestId('settings-page')).toBeVisible();
	await expect(page.getByRole('dialog', { name: 'Settings', exact: true })).toHaveCount(0);
	await expect(page.getByRole('heading', { name: 'Settings', exact: true })).toBeVisible();

	await page.reload();
	await expect(page.getByTestId('settings-page')).toBeVisible();
	await expect(page.getByRole('dialog', { name: 'Settings', exact: true })).toHaveCount(0);

	await page.context().route('**/api/**', async (route) => {
		const pathname = new URL(route.request().url()).pathname;
		if (pathname === '/api/settings') {
			await route.fulfill({ json: initialSettings });
			return;
		}
		if (pathname === '/api/notifications') {
			await route.fulfill({ json: { items: [], next_cursor: null } });
			return;
		}
		if (pathname === '/api/notifications/unread-count') {
			await route.fulfill({ json: { count: 0, latest_revision: 0 } });
			return;
		}
		await route.fallback();
	});
	const newTab = await page.context().newPage();
	try {
		await newTab.goto(new URL('/settings', page.url()).toString());
		await expect(newTab.getByTestId('settings-page')).toBeVisible();
		await expect(newTab.getByRole('dialog', { name: 'Settings', exact: true })).toHaveCount(0);
	} finally {
		await newTab.close();
		await page.context().unroute('**/api/**');
	}
});

test('Open full settings converts the overlay into the normal Settings route', async ({ page }) => {
	await page.setViewportSize({ width: 1280, height: 860 });
	await page.emulateMedia({ reducedMotion: 'no-preference' });
	await page.addInitScript(() => {
		type TransitionRecord = {
			phase: 'before' | 'after';
			surface: 'modal' | 'page' | null;
			width: number;
			height: number;
		};
		const records: TransitionRecord[] = [];
		Object.defineProperty(window, '__settingsTransitionRecords', {
			configurable: true,
			value: records
		});
		Object.defineProperty(window, '__settingsTransitionReady', {
			configurable: true,
			writable: true,
			value: Promise.resolve()
		});
		Object.defineProperty(document, 'startViewTransition', {
			configurable: true,
			value: (callback: () => Promise<void> | void) => {
				const capture = (phase: TransitionRecord['phase']): TransitionRecord => {
					const modal = document.querySelector<HTMLElement>(
						'[data-testid="settings-dialog-panel"]'
					);
					const page = document.querySelector<HTMLElement>(
						'[data-testid="settings-page-transition"]'
					);
					const element = modal ?? page;
					return {
						phase,
						surface: modal ? 'modal' : page ? 'page' : null,
						width: element?.getBoundingClientRect().width ?? 0,
						height: element?.getBoundingClientRect().height ?? 0
					};
				};

				records.push(capture('before'));
				const ready = Promise.resolve()
					.then(callback)
					.then(() => records.push(capture('after')))
					.then(() => undefined);
				(
					window as unknown as Window & { __settingsTransitionReady: Promise<void> }
				).__settingsTransitionReady = ready;
				return { ready, updateCallbackDone: ready, finished: ready, skipTransition() {} };
			}
		});
	});
	await openArticles(page);
	const originalUrl = page.url();
	const { dialog } = await openSettingsModal(page);

	await dialog.getByRole('button', { name: 'Open full settings', exact: true }).click();
	await page.evaluate(
		() =>
			(window as unknown as Window & { __settingsTransitionReady: Promise<void> })
				.__settingsTransitionReady
	);
	await expect(page).toHaveURL(/\/settings$/);
	await expect(page.getByRole('dialog', { name: 'Settings', exact: true })).toHaveCount(0);
	await expect(page.getByTestId('settings-page')).toBeVisible();
	await expect(page.getByTestId('settings-page')).toHaveAttribute('data-presentation', 'page');
	await expect(page.getByRole('heading', { name: 'Settings', exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'Collapse settings', exact: true }).click();
	await page.evaluate(
		() =>
			(window as unknown as Window & { __settingsTransitionReady: Promise<void> })
				.__settingsTransitionReady
	);
	await expect(page).toHaveURL(/\/settings$/);
	await expect(page.getByRole('dialog', { name: 'Settings', exact: true })).toBeVisible();
	await expect(page.getByTestId('settings-view')).toHaveAttribute('data-presentation', 'modal');
	await expect(page.getByTestId('articles-page')).toBeVisible();

	await page.goBack();
	await expect(page).toHaveURL(originalUrl);
	await expect(page.getByRole('dialog', { name: 'Settings', exact: true })).toHaveCount(0);
	await expect(page.getByTestId('articles-page')).toBeVisible();

	const transitions = await page.evaluate(
		() =>
			(
				window as unknown as Window & {
					__settingsTransitionRecords: Array<{
						phase: 'before' | 'after';
						surface: 'modal' | 'page' | null;
						width: number;
						height: number;
					}>;
				}
			).__settingsTransitionRecords
	);
	expect(transitions.map(({ phase, surface }) => `${phase}:${surface}`)).toEqual([
		'before:modal',
		'after:page',
		'before:page',
		'after:modal'
	]);
	for (const transition of transitions) {
		expect(transition.width).toBeGreaterThan(0);
		expect(transition.height).toBeGreaterThan(0);
	}
});

test('a settings mutation made in the modal is available on the full page', async ({ page }) => {
	await page.setViewportSize({ width: 1280, height: 860 });
	let persistedSettings = { ...initialSettings };
	await page.route('**/api/settings', async (route) => {
		if (route.request().method() === 'PATCH') {
			const body = route.request().postDataJSON();
			expect(body).toMatchObject({ default_max_depth: 3 });
			persistedSettings = { ...persistedSettings, ...body };
		}
		await route.fulfill({ json: persistedSettings });
	});

	await openArticles(page);
	const { dialog } = await openSettingsModal(page);
	await dialog.getByLabel('Default max depth').fill('3');
	await expect(dialog.getByTestId('settings-save-status')).toHaveText(/Unsaved changes/);
	await expect(dialog.getByTestId('settings-save-status')).toHaveText(/Changes saved/);
	await expect(dialog.getByRole('button', { name: /Save settings/i })).toHaveCount(0);

	await dialog.getByRole('button', { name: 'Open full settings', exact: true }).click();
	await expect(page.getByTestId('settings-page')).toBeVisible();
	await expect(page.getByLabel('Default max depth')).toHaveValue('3');
	await settleVisualPage(page);
});
