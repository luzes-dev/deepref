import type { Meta, StoryObj } from "@storybook/svelte";
import DesignTokensShowcase from "./components/DesignTokensShowcase.svelte";

const meta = {
	title: "Foundations/Design language",
	component: DesignTokensShowcase,
	parameters: {
		layout: "padded",
	},
} satisfies Meta<typeof DesignTokensShowcase>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};
