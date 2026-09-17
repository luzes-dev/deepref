import type { Meta, StoryObj } from "@storybook/svelte";
import SequenceShowcase from "./components/SequenceShowcase.svelte";
const meta = {
	title: "Primitives/Sequences",
	component: SequenceShowcase,
	parameters: { layout: "fullscreen" },
} satisfies Meta<typeof SequenceShowcase>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Gallery: Story = {};
