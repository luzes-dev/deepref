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
		await expect(page.getByLabel('Name')).toHaveValue('Evidence mapping protocol');
		await captureDarkViewport(page, 'plan-protocol.png');
	});

	test('Collect: imports', async ({ page }) => {
		await openWorkflow(page, '/discovery/imports', 'Imports');
		await expect(page.getByRole('heading', { name: 'Start an ingestion' })).toBeVisible();
		await captureDarkViewport(page, 'collect-imports.png');
	});

	test('Collect: articles', async ({ page }) => {
		await openWorkflow(page, '/articles', 'Articles');
		expect(
			await page
				.getByText('Effects of evidence mapping on review quality', { exact: true })
				.count()
		).toBeGreaterThan(0);
		await captureDarkViewport(page, 'collect-articles.png');
	});

	test('Collect: deduplication', async ({ page }) => {
		await openWorkflow(page, '/discovery/duplicates', 'Resolve duplicate records');
		await expect(
			page.getByText('Effects of evidence mapping on review quality', { exact: true })
		).toHaveCount(3);
		await captureDarkViewport(page, 'collect-deduplication.png');
	});

	test('Review: title and abstract screening', async ({ page }) => {
		await openWorkflow(page, '/screening/title-abstract', 'Screen reports');
		await expect(page.getByText('Focus mode', { exact: true })).toBeVisible();
		await expect(
			page.getByText(
				'An evidence-mapping workflow can make review decisions auditable and reproducible.'
			)
		).toBeVisible();
		await captureDarkViewport(page, 'review-title-abstract.png');
	});

	test('Review: full-text screening', async ({ page }) => {
		await openWorkflow(page, '/screening/full-text', 'Screen full text');
		await expect(page.getByText('Full-text decision', { exact: true })).toBeVisible();
		await expect(page.getByText('Attach a PDF to open the document viewer.')).toBeVisible();
		await captureDarkViewport(page, 'review-full-text.png');
	});

	test('Review: studies', async ({ page }) => {
		await openWorkflow(page, '/studies', 'Studies');
		await expect(page.getByText('No study groups yet.', { exact: true })).toBeVisible();
		await captureDarkViewport(page, 'review-studies.png');
	});

	test('Review: appraisal', async ({ page }) => {
		await openWorkflow(page, '/appraisal', 'Appraisal');
		await expect(page.getByText('No appraisal definitions', { exact: true })).toBeVisible();
		await captureDarkViewport(page, 'review-appraisal.png');
	});

	test('Review: extraction', async ({ page }) => {
		await openWorkflow(page, '/extraction', 'Extraction');
		await expect(page.getByTestId('extraction-study-empty')).toBeVisible();
		await expect(page.getByTestId('extraction-review-empty')).toBeVisible();
		await captureDarkViewport(page, 'review-extraction.png');
	});

	test('Operate: automations', async ({ page }) => {
		await openWorkflow(page, '/automations', 'Automation Center');
		await expect(page.getByText('project_maintenance.v1', { exact: true })).toBeVisible();
		await expect(page.getByRole('heading', { name: 'Recent runs' })).toBeVisible();
		await captureDarkViewport(page, 'operate-automations.png');
	});

	test('Operate: assistant', async ({ page }) => {
		await openWorkflow(page, '/assistant', 'Project Assistant');
		await expect(page.getByTestId('assistant-tool-get_project_protocol')).toBeVisible();
		await expect(page.getByTestId('assistant-tool-propose_screening_decision')).toBeVisible();
		await captureDarkViewport(page, 'operate-assistant.png');
	});
});
