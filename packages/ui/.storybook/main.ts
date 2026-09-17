import type { StorybookConfig } from "@storybook/svelte-vite";
import tailwindcss from "@tailwindcss/vite";
import path from "path";

const config: StorybookConfig = {
	stories: ["../stories/**/*.stories.@(js|ts|svelte)"],
	addons: [
		"@storybook/addon-essentials",
		"@storybook/addon-a11y",
		"@storybook/addon-themes",
		"@storybook/addon-interactions",
	],
	framework: {
		name: "@storybook/svelte-vite",
		options: {},
	},
	viteFinal: async (config) => {
		config.plugins = config.plugins || [];
		config.plugins.push(tailwindcss());

		config.resolve = config.resolve || {};
		config.resolve.alias = {
			...config.resolve.alias,
			"@deepref/ui": path.resolve(__dirname, "../src"),
		};

		return config;
	},
};

export default config;
