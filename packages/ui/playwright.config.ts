import { defineConfig } from "@playwright/test";
export default defineConfig({
	testDir: "./tests/visual",
	fullyParallel: true,
	workers: 2,
	use: {
		baseURL: "http://localhost:6006",
		reducedMotion: "reduce",
		locale: "en-US",
	},
	webServer: {
		command: "pnpm exec storybook dev -p 6006 --ci",
		url: "http://localhost:6006",
		reuseExistingServer: !process.env.CI,
	},
	snapshotPathTemplate: "{testDir}/__snapshots__/{projectName}/{arg}{ext}",
	expect: {
		timeout: 15000,
		toHaveScreenshot: {
			animations: "disabled",
			caret: "hide",
			maxDiffPixelRatio: 0.001,
		},
	},
	projects: [
		{
			name: "light-desktop",
			use: {
				viewport: { width: 1100, height: 1000 },
				colorScheme: "light",
			},
		},
		{
			name: "dark-desktop",
			use: {
				viewport: { width: 1100, height: 1000 },
				colorScheme: "dark",
			},
		},
		{
			name: "light-narrow",
			use: {
				viewport: { width: 390, height: 844 },
				colorScheme: "light",
			},
		},
		{
			name: "dark-narrow",
			use: { viewport: { width: 390, height: 844 }, colorScheme: "dark" },
		},
	],
});
