import { defineConfig, devices } from '@playwright/test';

const serverURL = 'http://localhost:4173';

const standardProject = {
	name: 'e2e',
	testMatch: '**/*.e2e.{ts,js}'
};

const visualUse = {
	baseURL: serverURL,
	locale: 'en-US',
	timezoneId: 'UTC',
	reducedMotion: 'reduce' as const
};

/**
 * The review matrix is kept here so the default config and the visual-only
 * compatibility config share one source of truth. The e2e project is explicit
 * in the package script, so ordinary E2E runs do not fan out across this matrix.
 */
export const deterministicVisualProjects = [
	{
		name: 'visual-dark-desktop-1440x1100',
		testMatch: '**/tests/visual/**/*.spec.ts',
		use: {
			...devices['Desktop Chrome'],
			...visualUse,
			viewport: { width: 1440, height: 1100 },
			colorScheme: 'dark' as const
		}
	},
	{
		name: 'visual-light-desktop-1440x1100',
		testMatch: '**/tests/visual/**/*.spec.ts',
		use: {
			...devices['Desktop Chrome'],
			...visualUse,
			viewport: { width: 1440, height: 1100 },
			colorScheme: 'light' as const
		}
	},
	{
		name: 'visual-dark-mobile-390x844',
		testMatch: '**/tests/visual/**/*.spec.ts',
		use: {
			...devices['Pixel 5'],
			...visualUse,
			viewport: { width: 390, height: 844 },
			colorScheme: 'dark' as const
		}
	},
	{
		name: 'visual-light-mobile-390x844',
		testMatch: '**/tests/visual/**/*.spec.ts',
		use: {
			...devices['Pixel 5'],
			...visualUse,
			viewport: { width: 390, height: 844 },
			colorScheme: 'light' as const
		}
	}
];

export default defineConfig({
	use: {
		baseURL: serverURL
	},
	webServer: {
		command: 'npm run build && npm run preview -- --host 0.0.0.0',
		port: 4173,
		reuseExistingServer: true
	},
	projects: [standardProject, ...deterministicVisualProjects]
});
