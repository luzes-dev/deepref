import type { Meta, StoryObj } from "@storybook/svelte";
import SupportingShowcase from "./components/SupportingShowcase.svelte";
const meta = {
	title: "Primitives/Supporting controls",
	component: SupportingShowcase,
	parameters: { layout: "fullscreen" },
} satisfies Meta<typeof SupportingShowcase>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Gallery: Story = {};
