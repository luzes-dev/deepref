import { expect, settleVisualPage, test } from './fixtures';
import { VISUAL_PROJECT_ID } from './api-fixture';

const workflows = [
	{
		name: 'full-text screening',
		path: '/screening/full-text',
		heading: 'Screen full text'
	},
	{
		name: 'title-and-abstract screening',
		path: '/screening/title-abstract',
		heading: 'Screen reports'
	}
] as const;

for (const workflow of workflows) {
	test(`AI-off status does not fetch proposals during ${workflow.name} startup`, async ({
		page,
		visualApi
	}) => {
		let releaseStatus!: () => void;
		let notifyStatusRequest!: () => void;
		const statusGate = new Promise<void>((resolve) => {
			releaseStatus = resolve;
		});
		const statusRequested = new Promise<void>((resolve) => {
			notifyStatusRequest = resolve;
		});

		await page.route('**/api/ai/status', async (route) => {
			notifyStatusRequest();
			await statusGate;
			await route.fulfill({
				status: 200,
				contentType: 'application/json',
				body: JSON.stringify({
					data: {
						assistant_available: false,
						configured: false,
						model: null,
						provider: null,
						suggestions_available: false
					}
				})
			});
		});

		try {
			await page.goto(`/projects/${VISUAL_PROJECT_ID}${workflow.path}`);
			await statusRequested;
			await expect(
				page.getByRole('heading', { name: workflow.heading, exact: true })
			).toBeVisible();
			await expect(
				page.getByRole('heading', {
					name: 'Effects of evidence mapping on review quality',
					exact: true
				})
			).toBeVisible();

			// Let the query observers process the report response while AI status remains pending.
			await page.evaluate(async () => {
				await Promise.resolve();
				await Promise.resolve();
			});
			expect(
				visualApi.requests.filter((request) => request.includes('/ai/proposals?'))
			).toEqual([]);
		} finally {
			releaseStatus();
		}

		await settleVisualPage(page);
		await expect(page.getByText('Get an AI suggestion', { exact: true })).toHaveCount(0);
		expect(visualApi.requests.filter((request) => request.includes('/ai/proposals?'))).toEqual(
			[]
		);
	});
}
