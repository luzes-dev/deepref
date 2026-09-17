import type { Meta, StoryObj } from "@storybook/svelte";
import CompositionShowcase from "./components/CompositionShowcase.svelte";
const meta = {
	title: "Patterns/Page composition",
	component: CompositionShowcase,
	parameters: { layout: "fullscreen" },
} satisfies Meta<typeof CompositionShowcase>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Workspace: Story = {};
export const Narrow: Story = {
	args: { constrained: true },
	parameters: { viewport: { defaultViewport: "mobile1" } },
};
