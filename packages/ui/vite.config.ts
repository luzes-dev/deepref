import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import path from "path";

export default defineConfig({
	plugins: [svelte()],
	resolve: {
		conditions: ["browser"],
		alias: {
			"@deepref/ui": path.resolve(__dirname, "./src"),
		},
	},
	test: {
		environment: "jsdom",
		setupFiles: ["./src/tests/setup.ts"],
		include: ["src/**/*.{test,spec}.{js,ts}"],
	},
});
