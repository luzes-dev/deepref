import { expect, test as base } from '@playwright/test';

/**
 * Playwright `test` for the mocked workspace specs. The shell asks `/api/me` who is signed in
 * before it renders, so every spec answers it as a deployment that does not require sign-in.
 * Routes a spec registers on the page take precedence over this context-level default.
 */
export const test = base.extend({
	context: async ({ context }, use) => {
		await context.route(/\/api\/me(?:\?.*)?$/, async (route) => {
			await route.fulfill({ json: { auth_enabled: false, user: null } });
		});
		await use(context);
	}
});

export { expect };
