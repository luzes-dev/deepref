import { expect, test as base, type Page, type Route } from '@playwright/test';
import { VISUAL_GET_ENDPOINTS, VISUAL_PROJECT_ID } from './api-fixture';

const FIXED_NOW = Date.parse('2026-01-15T12:00:00.000Z');

export type VisualApi = {
	readonly requests: readonly string[];
	readonly unhandledRequests: readonly string[];
	install(page: Page): Promise<void>;
};

function jsonResponse(route: Route, body: unknown, status = 200): Promise<void> {
	return route.fulfill({
		status,
		contentType: 'application/json',
		body: JSON.stringify(body) ?? 'null'
	});
}

type EndpointResponse = { body: unknown; status: number };

/**
 * Answer a request only when its path and canonical query match a modelled endpoint exactly.
 * Anything else, including a known path with a query the fixture has not modelled, is refused.
 */
function endpointResponse(url: URL, method: string): EndpointResponse {
	const query = url.searchParams.toString();
	const match =
		method === 'GET'
			? VISUAL_GET_ENDPOINTS.find(
					(endpoint) => endpoint.path === url.pathname && endpoint.query === query
				)
			: undefined;
	if (match) return { body: match.body, status: 200 };
	return {
		body: { detail: `No visual fixture for ${method} ${url.pathname}${url.search}` },
		status: 404
	};
}

async function installDeterminism(page: Page): Promise<void> {
	await page.addInitScript(
		({ fixedNow }) => {
			Date.now = () => fixedNow;
			let uuidCounter = 0;
			try {
				Object.defineProperty(globalThis.crypto, 'randomUUID', {
					configurable: true,
					value: () => {
						uuidCounter += 1;
						return `00000000-0000-4000-8000-${String(uuidCounter).padStart(12, '0')}`;
					}
				});
			} catch {
				// Some browser versions expose a non-configurable Crypto object. The fixture
				// does not rely on UUID generation, so determinism remains intact.
			}
		},
		{ fixedNow: FIXED_NOW }
	);
}

async function installMotionReset(page: Page): Promise<void> {
	await page.addStyleTag({
		content: `
			*, *::before, *::after {
				animation-delay: 0s !important;
				animation-duration: 0s !important;
				animation-iteration-count: 1 !important;
				transition-delay: 0s !important;
				transition-duration: 0s !important;
				scroll-behavior: auto !important;
			}
		`
	});
}

export async function settleVisualPage(page: Page): Promise<void> {
	await page.waitForLoadState('networkidle');
	await page.evaluate(async () => {
		await document.fonts.ready;
		await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
	});
}

function createVisualApi(): VisualApi {
	const requests: string[] = [];
	const unhandledRequests: string[] = [];

	return {
		get requests() {
			return [...requests];
		},
		get unhandledRequests() {
			return [...unhandledRequests];
		},
		async install(page: Page) {
			await installDeterminism(page);
			await page.emulateMedia({ reducedMotion: 'reduce' });
			await page.route('**/api/**', async (route) => {
				const request = route.request();
				const url = new URL(request.url());
				const method = request.method();
				const requestLabel = `${method} ${url.pathname}${url.search}`;
				requests.push(requestLabel);
				const response = endpointResponse(url, method);
				if (response.status === 404) unhandledRequests.push(requestLabel);
				await jsonResponse(route, response.body, response.status);
			});
			await page.addInitScript(() => {
				const applyColorScheme = () => {
					const root = document.documentElement;
					if (root) {
						root.classList.toggle(
							'dark',
							window.matchMedia('(prefers-color-scheme: dark)').matches
						);
					}
				};
				if (document.readyState === 'loading') {
					document.addEventListener('DOMContentLoaded', applyColorScheme, { once: true });
				} else {
					applyColorScheme();
				}
			});
			await page.goto(`/projects/${VISUAL_PROJECT_ID}/overview`);
			await settleVisualPage(page);
			await installMotionReset(page);
		}
	};
}

type VisualFixtures = {
	visualApi: VisualApi;
};

export const test = base.extend<VisualFixtures>({
	visualApi: [
		async ({ page }, use) => {
			const visualApi = createVisualApi();
			await visualApi.install(page);
			await use(visualApi);
			expect(visualApi.unhandledRequests).toEqual([]);
		},
		{ auto: true }
	]
});

export { expect } from '@playwright/test';

export async function captureViewport(page: Page, snapshotName: string): Promise<void> {
	await settleVisualPage(page);
	if (process.env.REVIEW_CAPTURE) {
		await page.screenshot({
			path: test.info().outputPath(snapshotName),
			scale: 'css',
			animations: 'disabled',
			caret: 'hide'
		});
		return;
	}
	await expect(page).toHaveScreenshot(snapshotName, {
		animations: 'disabled',
		caret: 'hide'
	});
}

export async function captureDarkViewport(page: Page, snapshotName: string): Promise<void> {
	if (!process.env.REVIEW_CAPTURE && !test.info().project.name.includes('-dark-')) return;
	await captureViewport(page, snapshotName);
}

export async function isMobileViewport(page: Page): Promise<boolean> {
	return page.evaluate(() => window.innerWidth < 768);
}
