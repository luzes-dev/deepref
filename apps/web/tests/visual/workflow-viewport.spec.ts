import { test, expect } from './fixtures';
import type { Page } from '@playwright/test';

async function openEditor(page: Page): Promise<void> {
	await page.goto('/projects/visual-project/automations');
	await page.getByTestId('automation-add-definition').click();
	await expect(page.getByTestId('workflow-editor')).toBeVisible();
	await expect(page.locator('[data-workflow-viewport]')).toHaveCount(1);
	await expect(page.locator('[data-workflow-node]').first()).toBeVisible();
	await expect(page.getByRole('slider', { name: 'Zoom', exact: true })).toBeVisible();
}

async function zoom(page: Page): Promise<number> {
	return page.locator('[data-workflow-viewport]').evaluate((element) => {
		const transform = getComputedStyle(element).transform;
		return transform === 'none' ? 1 : new DOMMatrixReadOnly(transform).a;
	});
}

async function expectSynchronizedZoom(page: Page): Promise<void> {
	await expect
		.poll(async () => {
			return page.locator('[data-testid="workflow-editor"]').evaluate((editor) => {
				const viewport = editor.querySelector('[data-workflow-viewport]');
				const slider = editor.querySelector<HTMLInputElement>('input[aria-label="Zoom"]');
				if (!viewport || !slider) return false;
				const scale = new DOMMatrixReadOnly(getComputedStyle(viewport).transform).a;
				return (
					Number(slider.value) === Math.round(scale * 100) &&
					editor.textContent?.includes(`${Math.round(scale * 100)}%`)
				);
			});
		})
		.toBe(true);
}

test('workflow viewport controls follow wheel and trackpad zoom', async ({ page }) => {
	await openEditor(page);
	const editor = page.locator('[data-workflow-area]');
	const box = await editor.boundingBox();
	if (!box) throw new Error('Editor bounds are missing');
	await page.mouse.move(box.x + box.width / 2, box.y + 30);
	const before = await zoom(page);
	await page.mouse.wheel(0, -120);
	await expect.poll(() => zoom(page)).toBeGreaterThan(before);
	await expectSynchronizedZoom(page);
	const afterWheel = await zoom(page);
	await editor.dispatchEvent('wheel', {
		bubbles: true,
		cancelable: true,
		ctrlKey: true,
		deltaY: -120,
		clientX: box.x + box.width / 2,
		clientY: box.y + 30
	});
	await expect.poll(() => zoom(page)).toBeGreaterThan(afterWheel);
	await expectSynchronizedZoom(page);
});

test('workflow Fit uses consistent bounds after zoom and respects reduced motion', async ({
	page
}) => {
	await page.emulateMedia({ reducedMotion: 'reduce' });
	await openEditor(page);
	const fit = page.getByRole('button', { name: 'Fit to view', exact: true });
	await fit.click();
	const fitted = await zoom(page);
	expect(fitted).toBeGreaterThanOrEqual(0.5);
	expect(fitted).toBeLessThanOrEqual(1.1);
	await page.getByRole('slider', { name: 'Zoom', exact: true }).fill('150');
	await expect.poll(() => zoom(page)).toBeCloseTo(1.5, 2);
	await page.locator('[data-workflow-viewport]').evaluate((viewport) => {
		if (!(viewport instanceof HTMLElement))
			throw new Error('Workflow viewport is not an HTML element');
		const scale = () => new DOMMatrixReadOnly(viewport.style.transform).a;
		const samples = new Set([scale()]);
		const observer = new MutationObserver(() => {
			samples.add(scale());
			viewport.setAttribute('data-transition-samples', JSON.stringify([...samples]));
			viewport.setAttribute('data-transition-count', String(samples.size));
		});
		observer.observe(viewport, { attributes: true, attributeFilter: ['style'] });
		viewport.addEventListener('stop-motion-capture', () => observer.disconnect(), {
			once: true
		});
	});
	await fit.click();
	await expect.poll(() => zoom(page)).toBeCloseTo(fitted, 2);
	await page.waitForTimeout(550);
	await expect(page.locator('[data-workflow-viewport]')).toHaveAttribute(
		'data-transition-count',
		'2'
	);
	await page.getByRole('button', { name: 'Zoom in', exact: true }).click();
	await expect.poll(() => zoom(page)).toBeCloseTo((Math.round(fitted * 100) + 10) / 100, 2);
	await page.waitForTimeout(550);
	await page.locator('[data-workflow-viewport]').dispatchEvent('stop-motion-capture');
	await expect(page.locator('[data-workflow-viewport]')).toHaveAttribute(
		'data-transition-count',
		'3'
	);
	await expectSynchronizedZoom(page);
});

test('workflow initial and manual Fit share the same upper zoom bound', async ({ page }) => {
	await page.setViewportSize({ width: 2400, height: 1200 });
	await openEditor(page);
	const initial = await zoom(page);
	expect(initial).toBeLessThanOrEqual(1.1);
	await page.getByRole('slider', { name: 'Zoom', exact: true }).fill('50');
	await expect.poll(() => zoom(page)).toBeCloseTo(0.5, 2);
	await page.getByRole('button', { name: 'Fit to view', exact: true }).click();
	await expect.poll(() => zoom(page)).toBeCloseTo(initial, 2);
	await expectSynchronizedZoom(page);
});
