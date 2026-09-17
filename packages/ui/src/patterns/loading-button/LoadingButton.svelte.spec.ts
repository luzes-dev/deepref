import { describe, it, expect } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/svelte";
import { createRawSnippet } from "svelte";
import LoadingButton from "./LoadingButton.svelte";
const children = createRawSnippet(() => ({
	render: () => "<span>Export records</span>",
}));
describe("LoadingButton", () => {
	it("retains its accessible name and announces busy state until work completes", async () => {
		const completion = Promise.withResolvers<void>();
		render(LoadingButton, {
			children,
			onClickPromise: () => completion.promise,
		});
		const button = screen.getByRole("button", { name: "Export records" });
		await fireEvent.click(button);
		expect(button).toBeDisabled();
		expect(button).toHaveAttribute("aria-busy", "true");
		expect(screen.queryByRole("status")).not.toBeInTheDocument();
		completion.resolve();
		await waitFor(() => expect(button).not.toBeDisabled());
		expect(button).toHaveAttribute("aria-busy", "false");
	});
});
