import type { Meta, StoryObj } from "@storybook/svelte";
import { StatePanel } from "../src";

const meta = {
	title: "Patterns/State panel",
	component: StatePanel,
	tags: ["autodocs"],
	argTypes: {
		state: {
			control: "select",
			options: ["empty", "loading", "error", "degraded", "success"],
		},
		title: {
			control: "text",
		},
		description: {
			control: "text",
		},
	},
} satisfies Meta<typeof StatePanel>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Empty: Story = {
	args: {
		state: "empty",
		title: "No records found",
		description: "Add records or adjust your filters to see results.",
	},
};

export const ErrorState: Story = {
	args: {
		state: "error",
		title: "Records could not be loaded",
		description:
			"Your connection was interrupted. Check the connection and try again.",
	},
};

export const Degraded: Story = {
	args: {
		state: "degraded",
		title: "Network synchronization degraded",
		description:
			"Live updates are temporarily unavailable. Displaying cached local data.",
	},
};

export const Loading: Story = {
	args: {
		state: "loading",
		title: "Preparing records",
		description: "This may take a moment. Your current work is saved.",
	},
};
export const Success: Story = {
	args: { state: "success", title: "All changes saved" },
};
