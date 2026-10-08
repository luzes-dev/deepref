import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import PaginationLoadMore from "./PaginationLoadMore.svelte";

describe("PaginationLoadMore component", () => {
	it("renders only the load more button when hasNextPage is true", () => {
		expect.hasAssertions();
		const onLoadMore = vi.fn();
		render(PaginationLoadMore, {
			hasNextPage: true,
			loadedCount: 25,
			onLoadMore,
		});

		expect(screen.queryByText(/Loaded 25/)).not.toBeInTheDocument();
		const button = screen.getByRole("button", { name: /load more/i });
		expect(button).toBeInTheDocument();
		expect(button).not.toBeDisabled();
	});

	it("triggers onLoadMore callback on click", async () => {
		expect.hasAssertions();
		const onLoadMore = vi.fn();
		render(PaginationLoadMore, {
			hasNextPage: true,
			loadedCount: 10,
			onLoadMore,
		});

		const button = screen.getByRole("button", { name: /load more/i });
		await fireEvent.click(button);
		expect(onLoadMore).toHaveBeenCalledTimes(1);
	});

	it("disables button and displays loading text while loading", () => {
		expect.hasAssertions();
		const onLoadMore = vi.fn();
		render(PaginationLoadMore, {
			hasNextPage: true,
			isLoading: true,
			loadedCount: 10,
			onLoadMore,
		});

		const button = screen.getByRole("button", { name: /loading more/i });
		expect(button).toBeInTheDocument();
		expect(button).toBeDisabled();
	});

	it("renders nothing when hasNextPage is false", () => {
		expect.hasAssertions();
		const onLoadMore = vi.fn();
		render(PaginationLoadMore, {
			hasNextPage: false,
			loadedCount: 42,
			label: "citations",
			onLoadMore,
		});

		expect(screen.queryByText(/Loaded 42/)).not.toBeInTheDocument();
		expect(
			screen.queryByText(/All citations loaded/),
		).not.toBeInTheDocument();
		expect(screen.queryByRole("button")).not.toBeInTheDocument();
	});
});
