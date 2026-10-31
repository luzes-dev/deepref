import { expect, test as base } from '@playwright/test';
import type { ProjectionStatusDto } from '../lib/api/generated/models/index.js';

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

/** Match the projection contract shared by graph, recommendations and status responses. */
export function readyProjection({
	projectId,
	revision
}: {
	projectId: string;
	revision: number;
}): ProjectionStatusDto {
	return {
		project_id: projectId,
		state: 'ready',
		requested_revision: revision,
		applied_revision: revision,
		revision,
		watermark: revision,
		lag: 0,
		last_success_at: '2026-01-01T00:00:00Z'
	};
}
