import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/svelte";
import { createRawSnippet } from "svelte";
import * as Avatar from "./avatar/index.js";
import * as Message from "./message/index.js";
import * as Bubble from "./bubble/index.js";
import * as Marker from "./marker/index.js";
import * as Attachment from "./attachment/index.js";

const textSnippet = (text: string) =>
	createRawSnippet(() => ({
		render: () => `<span>${text}</span>`,
	}));

describe("Avatar Primitives", () => {
	it("renders Avatar with fallback text", () => {
		const { container } = render(Avatar.Root, {
			props: {
				children: createRawSnippet(() => ({
					render: () => '<span data-slot="avatar-fallback">DR</span>',
				})),
			},
		});

		const avatar = container.querySelector('[data-slot="avatar"]');
		expect(avatar).toBeInTheDocument();
		expect(avatar).toHaveClass("rounded-full");
	});
});

describe("Message Primitives", () => {
	it('renders Message.Root with align="start" and align="end"', () => {
		const { container: c1 } = render(Message.Root, {
			props: {
				align: "start",
				children: textSnippet("Assistant Turn"),
			},
		});
		const root1 = c1.querySelector('[data-slot="message-root"]');
		expect(root1).toBeInTheDocument();
		expect(root1).toHaveAttribute("data-align", "start");
		expect(root1).toHaveClass("flex-row");

		const { container: c2 } = render(Message.Root, {
			props: {
				align: "end",
				children: textSnippet("User Turn"),
			},
		});
		const root2 = c2.querySelector('[data-slot="message-root"]');
		expect(root2).toBeInTheDocument();
		expect(root2).toHaveAttribute("data-align", "end");
		expect(root2).toHaveClass("flex-row-reverse");
	});

	it("renders Message.Avatar with anchor positioning", () => {
		const { container: c1 } = render(Message.Avatar, {
			props: {
				anchor: "top",
				children: textSnippet("Bot"),
			},
		});
		const avatar1 = c1.querySelector('[data-slot="message-avatar"]');
		expect(avatar1).toHaveAttribute("data-anchor", "top");
		expect(avatar1).toHaveClass("self-start");

		const { container: c2 } = render(Message.Avatar, {
			props: {
				anchor: "bottom",
				children: textSnippet("User"),
			},
		});
		const avatar2 = c2.querySelector('[data-slot="message-avatar"]');
		expect(avatar2).toHaveAttribute("data-anchor", "bottom");
		expect(avatar2).toHaveClass("self-end");
	});

	it("renders Message.Content, Header, Footer, and Group", () => {
		const { container: contentC } = render(Message.Content, {
			props: {
				children: textSnippet("Content Area"),
			},
		});
		expect(
			contentC.querySelector('[data-slot="message-content"]'),
		).toBeInTheDocument();

		const { container: headerC } = render(Message.Header, {
			props: {
				children: textSnippet("DeepRef Assistant • Just now"),
			},
		});
		expect(
			headerC.querySelector('[data-slot="message-header"]'),
		).toBeInTheDocument();
		expect(
			screen.getByText("DeepRef Assistant • Just now"),
		).toBeInTheDocument();

		const { container: footerC } = render(Message.Footer, {
			props: {
				children: textSnippet("124 tokens"),
			},
		});
		expect(
			footerC.querySelector('[data-slot="message-footer"]'),
		).toBeInTheDocument();
		expect(screen.getByText("124 tokens")).toBeInTheDocument();

		const { container: groupC } = render(Message.Group, {
			props: {
				children: textSnippet("Turns"),
			},
		});
		expect(
			groupC.querySelector('[data-slot="message-group"]'),
		).toBeInTheDocument();
	});
});

describe("Bubble Primitives", () => {
	it("renders Bubble.Root across default, muted, and outline variants", () => {
		const { container: cDefault } = render(Bubble.Root, {
			props: {
				variant: "default",
				children: textSnippet("User prompt"),
			},
		});
		const bDefault = cDefault.querySelector('[data-slot="bubble-root"]');
		expect(bDefault).toHaveAttribute("data-variant", "default");
		expect(bDefault).toHaveClass("bg-primary");

		const { container: cMuted } = render(Bubble.Root, {
			props: {
				variant: "muted",
				children: textSnippet("Assistant reply"),
			},
		});
		const bMuted = cMuted.querySelector('[data-slot="bubble-root"]');
		expect(bMuted).toHaveAttribute("data-variant", "muted");
		expect(bMuted).toHaveClass("bg-muted");

		const { container: cOutline } = render(Bubble.Root, {
			props: {
				variant: "outline",
				children: textSnippet("Proposal outline"),
			},
		});
		const bOutline = cOutline.querySelector('[data-slot="bubble-root"]');
		expect(bOutline).toHaveAttribute("data-variant", "outline");
		expect(bOutline).toHaveClass("border-border");
	});

	it("renders Bubble.Content, Group, and Reactions", () => {
		const { container: cContent } = render(Bubble.Content, {
			props: {
				children: textSnippet("Formatted prose"),
			},
		});
		expect(
			cContent.querySelector('[data-slot="bubble-content"]'),
		).toBeInTheDocument();

		const { container: cGroup } = render(Bubble.Group, {
			props: {
				children: textSnippet("Bubble stack"),
			},
		});
		expect(
			cGroup.querySelector('[data-slot="bubble-group"]'),
		).toBeInTheDocument();

		const { container: cReactions } = render(Bubble.Reactions, {
			props: {
				children: textSnippet("Reactions"),
			},
		});
		expect(
			cReactions.querySelector('[data-slot="bubble-reactions"]'),
		).toBeInTheDocument();
	});
});

describe("Marker Primitives", () => {
	it('renders Marker.Root with role="status" and aria-live="polite"', () => {
		const { container } = render(Marker.Root, {
			props: {
				variant: "shimmer",
				children: textSnippet("Searching evidence database..."),
			},
		});

		const marker = container.querySelector('[data-slot="marker-root"]');
		expect(marker).toBeInTheDocument();
		expect(marker).toHaveAttribute("role", "status");
		expect(marker).toHaveAttribute("aria-live", "polite");
		expect(marker).toHaveAttribute("data-variant", "shimmer");
		expect(
			screen.getByText("Searching evidence database..."),
		).toBeInTheDocument();
	});

	it("renders Marker.Content", () => {
		const { container } = render(Marker.Content, {
			props: {
				children: textSnippet("Assistant is thinking..."),
			},
		});

		const content = container.querySelector('[data-slot="marker-content"]');
		expect(content).toBeInTheDocument();
		expect(
			screen.getByText("Assistant is thinking..."),
		).toBeInTheDocument();
	});
});

describe("Attachment Primitives", () => {
	it("renders non-interactive Attachment.Root as div", () => {
		const { container } = render(Attachment.Root, {
			props: {
				children: textSnippet("Protocol PDF"),
			},
		});

		const pill = container.querySelector('[data-slot="attachment-root"]');
		expect(pill).toBeInTheDocument();
		expect(pill?.tagName).toBe("DIV");
	});

	it("renders interactive Attachment.Root as anchor when href is passed", () => {
		const { container } = render(Attachment.Root, {
			props: {
				href: "/documents/protocol.pdf",
				children: textSnippet("Protocol PDF"),
			},
		});

		const link = container.querySelector('[data-slot="attachment-root"]');
		expect(link).toBeInTheDocument();
		expect(link?.tagName).toBe("A");
		expect(link).toHaveAttribute("href", "/documents/protocol.pdf");
	});

	it("renders Attachment.Preview and Attachment.Name", () => {
		const { container: previewC } = render(Attachment.Preview, {
			props: {
				children: textSnippet("PDF"),
			},
		});
		expect(
			previewC.querySelector('[data-slot="attachment-preview"]'),
		).toBeInTheDocument();

		const { container: nameC } = render(Attachment.Name, {
			props: {
				children: textSnippet("Cochrane_Review_2026.pdf"),
			},
		});
		const name = nameC.querySelector('[data-slot="attachment-name"]');
		expect(name).toBeInTheDocument();
		expect(name).toHaveClass("truncate");
		expect(
			screen.getByText("Cochrane_Review_2026.pdf"),
		).toBeInTheDocument();
	});
});
