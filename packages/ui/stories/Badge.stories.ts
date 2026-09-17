import type { Meta, StoryObj } from "@storybook/svelte";
import BadgeShowcase from "./components/BadgeShowcase.svelte";

const meta = {
	title: "Primitives/Status",
	component: BadgeShowcase,
	parameters: {
		layout: "padded",
	},
} satisfies Meta<typeof BadgeShowcase>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Gallery: Story = {};
