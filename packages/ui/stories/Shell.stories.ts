import type { Meta, StoryObj } from "@storybook/svelte";
import ShellShowcase from "./components/ShellShowcase.svelte";
const meta = {
	title: "Patterns/Responsive shell",
	component: ShellShowcase,
	parameters: { layout: "fullscreen" },
} satisfies Meta<typeof ShellShowcase>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Workspace: Story = {};
export const Narrow: Story = {
	parameters: { viewport: { defaultViewport: "mobile1" } },
};
