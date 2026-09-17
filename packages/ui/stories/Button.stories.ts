import type { Meta, StoryObj } from "@storybook/svelte";
import ButtonShowcase from "./components/ButtonShowcase.svelte";

const meta = {
	title: "Primitives/Actions",
	component: ButtonShowcase,
	parameters: {
		layout: "padded",
	},
} satisfies Meta<typeof ButtonShowcase>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Gallery: Story = {};
