import { expect, test, captureDarkViewport, settleVisualPage } from './fixtures';
import { runSeriousCriticalAxe } from './axe';
import type { Page } from '@playwright/test';

const projectId = 'visual-project';
const basePath = `/projects/${projectId}`;

async function assertThemeContract(page: Page): Promise<void> {
	const projectName = test.info().project.name;
	const expected = projectName.includes('-dark-') ? 'dark' : 'light';
	const theme = await page.evaluate(() => ({
		darkClass: document.documentElement.classList.contains('dark'),
		colorScheme: getComputedStyle(document.documentElement).colorScheme
	}));
	expect(theme.darkClass).toBe(expected === 'dark');
	expect(theme.colorScheme).toContain(expected);
}

async function openWorkflow(page: Page, path: string, heading: string): Promise<void> {
	await page.goto(`${basePath}${path}`);
	await settleVisualPage(page);
	await expect(page.getByRole('heading', { name: heading, exact: true })).toBeVisible();
	await assertThemeContract(page);
	const violations = await runSeriousCriticalAxe(page);
	expect(violations, JSON.stringify(violations, null, 2)).toEqual([]);
}

test.describe('DeepRef workflow family visual coverage', () => {
	test('Plan: protocol editor', async ({ page }) => {
		await openWorkflow(page, '/protocol', 'Review protocol');
		await expect(
			page.getByRole('heading', { name: 'Evidence mapping protocol', exact: true })
		).toBeVisible();
		await expect(page.getByRole('button', { name: 'Amend published version' })).toBeEnabled();
		await captureDarkViewport(page, 'plan-protocol.png');
	});

	test('Collect: imports', async ({ page }) => {
		await openWorkflow(page, '/discovery/imports', 'Imports');
		await captureDarkViewport(page, 'collect-imports.png');
	});

	test('Collect: articles', async ({ page }) => {
		await openWorkflow(page, '/articles', 'Articles');
		await captureDarkViewport(page, 'collect-articles.png');
	});

	test('Collect: deduplication', async ({ page }) => {
		await openWorkflow(page, '/discovery/duplicates', 'Resolve duplicate records');
		await expect(page.getByRole('list', { name: 'Source identifiers' })).toBeVisible();
		await expect(page.getByRole('list', { name: 'Candidate identifiers' })).toBeVisible();
		await expect(page.getByRole('button', { name: 'Accept candidate' })).toBeEnabled();
		await captureDarkViewport(page, 'collect-deduplication.png');
	});

	test('Review: title and abstract screening', async ({ page }) => {
		await openWorkflow(page, '/screening/title-abstract', 'Screen reports');
		await expect(page.getByRole('button', { name: 'Include', exact: true })).toBeEnabled();
		await expect(
			page.getByText(
				'An evidence-mapping workflow can make review decisions auditable and reproducible.'
			)
		).toBeVisible();
		await captureDarkViewport(page, 'review-title-abstract.png');
	});

	test('Review: full-text screening', async ({ page }) => {
		await openWorkflow(page, '/screening/full-text', 'Screen full text');
		await captureDarkViewport(page, 'review-full-text.png');
	});

	test('Review: studies', async ({ page }) => {
		await openWorkflow(page, '/studies', 'Studies');
		await expect(page.getByRole('textbox', { name: 'New study title' })).toBeVisible();
		await captureDarkViewport(page, 'review-studies.png');
	});

	test('Review: appraisal', async ({ page }) => {
		await openWorkflow(page, '/appraisal', 'Appraisal');
		await expect(
			page
				.getByRole('navigation', { name: 'Reports' })
				.or(page.getByRole('combobox', { name: 'Report to appraise' }))
		).toBeVisible();
		await captureDarkViewport(page, 'review-appraisal.png');
	});

	test('Review: extraction', async ({ page }) => {
		await openWorkflow(page, '/extraction', 'Extraction');
		await expect(page.getByTestId('extraction-study-empty').first()).toBeVisible();
		await page.getByRole('button', { name: /^Fields/ }).click();
		await expect(page.getByRole('textbox', { name: 'Label', exact: true })).toBeVisible();
		await captureDarkViewport(page, 'review-extraction.png');
	});

	test('Operate: automations', async ({ page }) => {
		await openWorkflow(page, '/automations', 'Automations');
		const list = page.getByTestId('automation-list');
		await expect(list).toBeVisible();
		await expect(
			list.getByRole('link', { name: 'Project maintenance', exact: true })
		).toBeVisible();
		await expect(page.getByTestId('automation-row')).toHaveCount(1);
		await captureDarkViewport(page, 'operate-automations.png');

		await page.getByTestId('new-automation').click();
		const gallery = page.getByTestId('template-gallery');
		await expect(gallery).toBeVisible();
		await expect(gallery.getByText('Refresh project metrics', { exact: true })).toBeVisible();
		await expect(page.getByTestId('start-from-scratch')).toBeVisible();
	});

	test('Operate: automation editor', async ({ page }) => {
		await openWorkflow(page, '/automations', 'Automations');
		await page.getByRole('link', { name: 'Project maintenance', exact: true }).click();
		await expect(page).toHaveURL(new RegExp(`${basePath}/automations/visual-workflow$`));
		await expect(page.getByTestId('automation-builder')).toBeVisible();
		await expect(page.getByTestId('builder-header')).toBeVisible();
		await settleVisualPage(page);
		const violations = await runSeriousCriticalAxe(page);
		expect(violations, JSON.stringify(violations, null, 2)).toEqual([]);
		await captureDarkViewport(page, 'operate-automation-editor.png');
	});

	test('Operate: assistant', async ({ page }) => {
		await openWorkflow(page, '/assistant', 'New conversation');
		await expect(page.getByTestId('assistant-feed')).toBeVisible();
		await page.getByTestId('assistant-sessions-toggle').click();
		await expect(
			page.getByRole('dialog', { name: 'Conversations', exact: true })
		).toBeVisible();
		await page.keyboard.press('Escape');
		await expect(page.getByRole('dialog', { name: 'Conversations', exact: true })).toBeHidden();
		await expect(page.getByTestId('assistant-sessions-toggle')).toBeFocused();
		await captureDarkViewport(page, 'operate-assistant.png');
	});
});
