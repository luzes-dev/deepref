import { defineConfig } from "jsrepo";

export default defineConfig({
	registries: ["@ieedan/shadcn-svelte-extras"],
	paths: {
		ui: "src/primitives",
		component: "src/patterns",
		block: "src/patterns",
		hook: "src/internal/hooks",
		action: "src/internal/actions",
		util: "src/internal/utils",
		lib: "src",
	},
});
