import type { TransitionConfig } from "svelte/transition";

export type Options = {
	speed: number;
	delay: number;
	onComplete?: () => void;
};

export const typewriter = (
	node: HTMLElement,
	{ speed = 1, delay = 0, onComplete }: Partial<Options>,
) => {
	const text = node.textContent ?? "";
	const reducedMotion = window.matchMedia(
		"(prefers-reduced-motion: reduce)",
	).matches;
	const duration = reducedMotion ? 0 : text.length / (speed * 0.01);

	return {
		delay: reducedMotion ? 0 : delay,
		duration,
		tick: (t) => {
			const i = Math.trunc(text.length * t);
			node.textContent = text.slice(0, i);
			if (node.textContent.length === text.length) onComplete?.();
		},
	} satisfies TransitionConfig;
};
