import type { Meta, StoryObj } from "@storybook/svelte";
import { expect, userEvent, within, waitFor } from "@storybook/test";
import OverlaysShowcase from "./components/OverlaysShowcase.svelte";
const meta = {
	title: "Primitives/Overlays",
	component: OverlaysShowcase,
	parameters: { layout: "fullscreen" },
} satisfies Meta<typeof OverlaysShowcase>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Gallery: Story = {};
export const DialogKeyboard: Story = {
	play: async ({ canvasElement }) => {
		const canvas = within(canvasElement);
		const body = within(canvasElement.ownerDocument.body);
		const trigger = canvas.getByRole("button", { name: "Edit record" });
		await userEvent.click(trigger);
		await expect(
			await body.findByRole("dialog", { name: "Edit record details" }),
		).toBeVisible();
		await userEvent.keyboard("{Escape}");
		await waitFor(() =>
			expect(body.queryByRole("dialog")).not.toBeInTheDocument(),
		);
		await expect(trigger).toHaveFocus();
	},
};
export const MenuKeyboard: Story = {
	play: async ({ canvasElement }) => {
		const canvas = within(canvasElement);
		const body = within(canvasElement.ownerDocument.body);
		await userEvent.click(
			canvas.getByRole("button", { name: "Record actions" }),
		);
		await expect(
			await body.findByRole("menuitem", { name: "Archive" }),
		).toHaveAttribute("data-disabled");
		await userEvent.keyboard("{Home}{Enter}");
		await expect(canvas.getByRole("status")).toHaveTextContent(
			"Duplicate selected",
		);
	},
};
export const LongDialog: Story = {
	args: { longContent: true },
	play: async ({ canvasElement }) => {
		await userEvent.click(
			within(canvasElement).getByRole("button", { name: "Edit record" }),
		);
	},
};
