import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import ToolCallCard from "./ToolCallCard.svelte";
import ProposalCard from "./ProposalCard.svelte";

describe("ToolCallCard", () => {
	it("renders tool name and completed status by default", () => {
		render(ToolCallCard, {
			toolName: "get_report",
			status: "completed",
		});

		const card = screen.getByRole("button");
		expect(card).toBeInTheDocument();
		expect(screen.getByText("get_report")).toBeInTheDocument();
		expect(screen.getByText("Completed")).toBeInTheDocument();
	});

	it("renders running status with duration", () => {
		render(ToolCallCard, {
			toolName: "screen_candidates",
			status: "running",
			durationMs: 450,
		});

		expect(screen.getByText("screen_candidates")).toBeInTheDocument();
		expect(screen.getByText("Running")).toBeInTheDocument();
		expect(screen.getByText("450ms")).toBeInTheDocument();
	});

	it("renders failed status with custom duration string", () => {
		render(ToolCallCard, {
			toolName: "extract_entities",
			status: "failed",
			duration: "1.2s",
		});

		expect(screen.getByText("extract_entities")).toBeInTheDocument();
		expect(screen.getByText("Failed")).toBeInTheDocument();
		expect(screen.getByText("1.2s")).toBeInTheDocument();
	});

	it("toggles collapsible details when clicked", async () => {
		render(ToolCallCard, {
			toolName: "deduplicate_cluster",
			args: { clusterId: "cluster-99", threshold: 0.85 },
			outputSummary: "Found 3 duplicates",
			open: false,
		});

		const toggleButton = screen.getByRole("button");
		expect(toggleButton).toHaveAttribute("aria-expanded", "false");
		expect(screen.queryByText(/cluster-99/i)).not.toBeInTheDocument();

		await fireEvent.click(toggleButton);
		expect(toggleButton).toHaveAttribute("aria-expanded", "true");
		expect(screen.getByText(/cluster-99/i)).toBeInTheDocument();
		expect(screen.getByText(/Found 3 duplicates/i)).toBeInTheDocument();

		await fireEvent.click(toggleButton);
		expect(toggleButton).toHaveAttribute("aria-expanded", "false");
	});

	it("renders without outer Bubble when bubble is false", () => {
		const { container } = render(ToolCallCard, {
			toolName: "fetch_metadata",
			bubble: false,
		});

		const card = container.querySelector('[data-slot="tool-call-card"]');
		expect(card).toBeInTheDocument();
		expect(card?.getAttribute("data-variant")).toBeNull();
	});
});

describe("ProposalCard", () => {
	it("renders proposal kind, summary, target ID, and review link", () => {
		render(ProposalCard, {
			kind: "propose_screening_decision",
			summary:
				"Include study based on randomized controlled trial design",
			targetId: "rep-uuid-1234",
			targetLabel: "Report ID",
			reviewHref: "/projects/p1/screening",
			reviewLabel: "Review Screening",
			confidence: 0.94,
		});

		expect(screen.getByText("Screening Decision")).toBeInTheDocument();
		expect(
			screen.getByText(
				"Include study based on randomized controlled trial design",
			),
		).toBeInTheDocument();
		expect(screen.getByText("Report ID:")).toBeInTheDocument();
		expect(screen.getByText("rep-uuid-1234")).toBeInTheDocument();
		expect(screen.getByText("94% confidence")).toBeInTheDocument();

		const reviewLink = screen.getByRole("link", {
			name: /Review Screening/i,
		});
		expect(reviewLink).toBeInTheDocument();
		expect(reviewLink).toHaveAttribute("href", "/projects/p1/screening");
	});

	it("invokes onReviewClick callback when review button is clicked", async () => {
		const onReviewClick = vi.fn();
		render(ProposalCard, {
			kind: "extraction",
			summary: "Extract sample size 450",
			reviewHref: "/review-queue",
			onReviewClick,
		});

		const link = screen.getByRole("link", { name: /Review in Queue/i });
		await fireEvent.click(link);
		expect(onReviewClick).toHaveBeenCalledTimes(1);
	});

	it("renders without outer Bubble when bubble is false", () => {
		const { container } = render(ProposalCard, {
			kind: "duplicate_merge",
			summary: "Merge two identical reports",
			bubble: false,
		});

		const card = container.querySelector('[data-slot="proposal-card"]');
		expect(card).toBeInTheDocument();
		expect(card?.getAttribute("data-variant")).toBeNull();
	});
});
