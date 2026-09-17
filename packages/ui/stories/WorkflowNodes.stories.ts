import type { Meta, StoryObj } from "@storybook/svelte";
import WorkflowNodeShowcase from "./components/WorkflowNodeShowcase.svelte";
const meta = {
	title: "Patterns/Workflow nodes",
	component: WorkflowNodeShowcase,
	parameters: {
		layout: "fullscreen",
		docs: {
			description: {
				component:
					"Renderer-independent node surfaces. Compose semantic labels, descriptions and controls; the application workflow editor owns graph mechanics.",
			},
		},
	},
} satisfies Meta<typeof WorkflowNodeShowcase>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Presentation: Story = {};
