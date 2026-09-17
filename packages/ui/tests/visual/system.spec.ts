import { test, expect } from "@playwright/test";

const targets = [
	["patterns-workflow-nodes--presentation", "Workflow node presentation"],
	["foundations-design-language--default", "A workspace for careful reading"],
	["primitives-actions--gallery", "Actions"],
	["primitives-forms--validation", "Form controls"],
	["primitives-status--gallery", "Status and feedback"],
	["primitives-table--interactive", "Dense records"],
	[
		"patterns-page-composition--workspace",
		"A shared collection with a deliberately long descriptive title",
	],
	["primitives-overlays--gallery", "Overlays"],
];
for (const [id, heading] of targets) {
	test(id, async ({ page }, testInfo) => {
		const theme = testInfo.project.name.startsWith("dark")
			? "dark"
			: "light";
		await page.goto(
			`/iframe.html?id=${id}&viewMode=story&globals=theme:${theme}`,
		);
		await expect(
			page.getByRole("heading", { name: heading, exact: true }),
		).toBeVisible();
		await page.evaluate(() => document.fonts.ready);
		await expect(page.locator("html")).toHaveClass(
			theme === "dark" ? /dark/ : /^(?!.*dark)/,
		);
		expect(
			await page.evaluate(
				() => document.documentElement.scrollWidth <= innerWidth,
			),
		).toBe(true);
		if (id.includes("overlays")) {
			await page.getByRole("button", { name: "Edit record" }).click();
			await expect(
				page.getByRole("dialog", { name: "Edit record details" }),
			).toBeVisible();
		}
		if (process.env.REVIEW_CAPTURE)
			await page.screenshot({
				path: testInfo.outputPath("review.png"),
				fullPage: true,
				animations: "disabled",
			});
		else
			await expect(page).toHaveScreenshot(`${id}.png`, {
				fullPage: true,
			});
	});
}

test("dialog, sheet, drawer, menu and table keyboard contracts", async ({
	page,
}, testInfo) => {
	const theme = testInfo.project.name.startsWith("dark") ? "dark" : "light";
	await page.goto(
		`/iframe.html?id=primitives-overlays--gallery&viewMode=story&globals=theme:${theme}`,
	);
	for (const name of ["Edit record", "Open inspector", "Open drawer"]) {
		const trigger = page.getByRole("button", { name, exact: true });
		await trigger.click();
		await expect(page.getByRole("dialog")).toBeVisible();
		await page.keyboard.press("Escape");
		await expect(page.getByRole("dialog")).toBeHidden();
		await expect(trigger).toBeFocused();
	}
	await page.getByRole("button", { name: "Record actions" }).click();
	await page.keyboard.press("Home");
	await page.keyboard.press("Enter");
	await expect(page.getByRole("status")).toHaveText("Duplicate selected");
	await page.goto(
		`/iframe.html?id=primitives-table--interactive&viewMode=story&globals=theme:${theme}`,
	);
	await page
		.getByRole("textbox", { name: "Filter records" })
		.fill("no matching title");
	await expect(page.getByRole("cell")).toHaveText(
		"No records match “no matching title”. Try a shorter search.",
	);
});

test("long dialog retains reachable footer at a short height", async ({
	page,
}) => {
	await page.setViewportSize({ width: 390, height: 500 });
	await page.goto(
		"/iframe.html?id=primitives-overlays--long-dialog&viewMode=story",
	);
	const save = page.getByRole("button", { name: "Save changes" });
	await save.scrollIntoViewIfNeeded();
	await expect(save).toBeInViewport();
	await save.click();
	await expect(page.getByRole("dialog")).toBeHidden();
});

test("tabs, command search and supporting controls", async ({ page }) => {
	await page.goto(
		"/iframe.html?id=primitives-navigation--gallery&viewMode=story",
	);
	await page.getByRole("tab", { name: "Details", exact: true }).click();
	await page.keyboard.press("ArrowRight");
	await expect(
		page.getByRole("tab", { name: "History and activity" }),
	).toHaveAttribute("aria-selected", "true");
	await page.getByRole("button", { name: "Find an action" }).click();
	await expect(
		page.getByRole("dialog", { name: "Find an action" }),
	).toBeVisible();
	await page.getByRole("combobox", { name: "Search actions" }).fill("Export");
	await page.keyboard.press("ArrowDown");
	await page.keyboard.press("Enter");
	await expect(page.getByRole("status")).toHaveText("Export selected");
	await page.goto(
		"/iframe.html?id=primitives-supporting-controls--gallery&viewMode=story",
	);
	const amount = page.getByRole("slider", { name: "Threshold" });
	await amount.focus();
	await page.keyboard.press("ArrowRight");
	await expect(amount).toHaveAttribute("aria-valuenow", "41");
	const input = page.getByRole("spinbutton", { name: "Batch size" });
	await input.fill("12");
	await page.getByRole("button", { name: "Increase" }).click();
	await expect(input).toHaveValue("13");
});

test("tag suggestions keep focus and expose only mounted options", async ({
	page,
}) => {
	await page.goto(
		"/iframe.html?id=primitives-supporting-controls--gallery&viewMode=story",
	);
	const input = page.getByRole("combobox", { name: "Labels" });
	await expect(input).not.toHaveAttribute("aria-activedescendant");
	await input.focus();
	await expect(input).toHaveAttribute("aria-expanded", "true");
	await page.keyboard.press("Escape");
	await expect(input).toBeFocused();
	await expect(input).not.toHaveAttribute("aria-activedescendant");
	await input.fill("Draft");
	await page.keyboard.press("Enter");
	await expect(
		page.getByRole("button", { name: "Remove Draft" }),
	).toBeVisible();
	await page.getByRole("button", { name: "Remove Draft" }).click();
	await expect(
		page.getByRole("button", { name: "Remove Draft" }),
	).toHaveCount(0);
});

test("stepper announces the current step", async ({ page }) => {
	await page.goto(
		"/iframe.html?id=primitives-sequences--gallery&viewMode=story",
	);
	await expect(
		page.getByRole("button", { name: "1 Choose records" }),
	).toHaveAttribute("aria-current", "step");
	await page.getByRole("button", { name: "2 Review options" }).click();
	await expect(
		page.getByRole("button", { name: "2 Review options" }),
	).toHaveAttribute("aria-current", "step");
});

test("responsive shell exposes close controls and a usable modal", async ({
	page,
}, testInfo) => {
	const theme = testInfo.project.name.startsWith("dark") ? "dark" : "light";
	await page.goto(
		`/iframe.html?id=patterns-responsive-shell--workspace&viewMode=story&globals=theme:${theme}`,
	);
	const toggle = page.getByRole("button", { name: "Toggle Sidebar" });
	await toggle.click();
	if (testInfo.project.name.includes("narrow")) {
		const sidebar = page.getByRole("dialog", {
			name: "Sidebar",
			exact: true,
		});
		await expect(sidebar).toBeVisible();
		await sidebar
			.getByRole("button", { name: "Close", exact: true })
			.click();
		await expect(sidebar).toBeHidden();
	} else {
		await expect(
			page.locator('[data-slot="sidebar"][data-state="collapsed"]'),
		).toBeVisible();
	}
	await page
		.getByRole("button", { name: "Add a record", exact: true })
		.click();
	const modal = page.getByRole("dialog", {
		name: "A responsive modal",
		exact: true,
	});
	await expect(modal).toBeVisible();
	await modal.getByRole("button", { name: "Cancel", exact: true }).click();
	await expect(modal).toBeHidden();
});
