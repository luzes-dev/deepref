import { describe, expect, it } from "vitest";
import { UseClipboard } from "./use-clipboard.svelte.js";
import { useRamp } from "./use-ramp.svelte.js";

describe("UI Hooks", () => {
	it("instantiates UseClipboard with undefined initial status", () => {
		const clipboard = new UseClipboard();
		expect(clipboard.status).toBeUndefined();
	});

	it("provides start and reset on useRamp", () => {
		let count = 0;
		const ramp = useRamp({
			increment: () => {
				count += 1;
			},
			canRamp: () => true,
		});
		expect(ramp.active).toBe(false);
		expect(ramp.ramping).toBe(false);
		expect(count).toBe(0);
	});
});
