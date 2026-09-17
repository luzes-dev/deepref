import type { Meta, StoryObj } from "@storybook/svelte";
import FormControlsShowcase from "./components/FormControlsShowcase.svelte";

const meta = {
	title: "Primitives/Forms",
	component: FormControlsShowcase,
	parameters: {
		layout: "padded",
	},
} satisfies Meta<typeof FormControlsShowcase>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Gallery: Story = {};

export const Validation: Story = { args: { invalid: true } };
export const Disabled: Story = { args: { disabled: true } };
export const Narrow: Story = {
	parameters: { viewport: { defaultViewport: "mobile1" } },
	args: { invalid: true },
};
