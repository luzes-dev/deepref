import type { Meta, StoryObj } from "@storybook/svelte";
import DataTableShowcase from "./components/DataTableShowcase.svelte";

const meta = {
	title: "Primitives/Table",
	component: DataTableShowcase,
	parameters: {
		layout: "padded",
	},
} satisfies Meta<typeof DataTableShowcase>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Interactive: Story = {};
