import { defineConfig } from '@playwright/test';
import baseConfig, { deterministicVisualProjects } from './playwright.config';

/**
 * Keep this entrypoint for focused visual commands while sharing the matrix
 * with the default config. It intentionally omits the ordinary E2E project.
 */
export default defineConfig({
	...baseConfig,
	testDir: './tests/visual',
	testMatch: '**/*.spec.ts',
	fullyParallel: true,
	timeout: 30_000,
	snapshotPathTemplate: '{testDir}/__snapshots__/{projectName}/{testFilePath}/{arg}{ext}',
	expect: {
		timeout: 5_000,
		toHaveScreenshot: {
			animations: 'disabled',
			caret: 'hide'
		}
	},
	use: {
		trace: 'retain-on-failure'
	},
	projects: deterministicVisualProjects
});
