import { defineConfig } from '@playwright/test';

export default defineConfig({
	webServer: {
		command: 'npm run build && npm run preview -- --host 0.0.0.0',
		port: 4173,
		reuseExistingServer: true
	},
	testMatch: '**/*.e2e.{ts,js}'
});
