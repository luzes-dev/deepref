import type { Meta, StoryObj } from "@storybook/svelte";
import { expect, userEvent, within } from "@storybook/test";
import NavigationShowcase from "./components/NavigationShowcase.svelte";
const meta = {
	title: "Primitives/Navigation",
	component: NavigationShowcase,
	parameters: { layout: "fullscreen" },
} satisfies Meta<typeof NavigationShowcase>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Gallery: Story = {};
export const Keyboard: Story = {
	play: async ({ canvasElement }) => {
		const canvas = within(canvasElement);
		await userEvent.click(canvas.getByRole("tab", { name: "Details" }));
		await userEvent.keyboard("{ArrowRight}");
		await expect(
			canvas.getByRole("tab", { name: "History and activity" }),
		).toHaveAttribute("aria-selected", "true");
		await userEvent.click(
			canvas.getByRole("button", { name: "Find an action" }),
		);
		const body = within(canvasElement.ownerDocument.body);
		await userEvent.type(
			await body.findByRole("combobox", { name: "Search actions" }),
			"Export",
		);
		await userEvent.keyboard("{ArrowDown}{Enter}");
		await expect(canvas.getByRole("status")).toHaveTextContent(
			"Export selected",
		);
	},
};
