import type { Preview } from "@storybook/svelte";
import { withThemeByClassName } from "@storybook/addon-themes";
import "./preview.css";

const preview: Preview = {
	parameters: {
		options: {
			storySort: { order: ["Foundations", "Primitives", "Patterns"] },
		},
		controls: {
			matchers: {
				color: /(background|color)$/i,
				date: /Date$/i,
			},
		},
		backgrounds: { disable: true },
	},
	decorators: [
		withThemeByClassName({
			themes: {
				light: "",
				dark: "dark",
			},
			defaultTheme: "light",
		}),
	],
};

export default preview;
