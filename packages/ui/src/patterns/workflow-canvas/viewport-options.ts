/**
 * Shared viewport bounds for the workflow canvas and its controls.
 *
 * Keeping the values together prevents the initial fit and the user-facing
 * fit control from drifting apart as the canvas evolves.
 */
export const WORKFLOW_FIT_VIEW_OPTIONS = {
	padding: 0.08,
	minZoom: 0.5,
	maxZoom: 1.1,
} as const;

/**
 * Interactive zoom limits for the canvas. These are intentionally wider than
 * the fit range so users can inspect a fitted workflow at a closer scale.
 */
export const WORKFLOW_ZOOM_LIMITS = {
	minZoom: 0.5,
	maxZoom: 2,
} as const;

/**
 * Builder canvas. A phone-sized screen has to show a whole automation on open,
 * so the fit may go much further out than the workflow overview does. A block
 * is never fitted larger than 100%.
 */
export const BUILDER_FIT_VIEW_OPTIONS = {
	padding: 0.12,
	minZoom: 0.2,
	maxZoom: 1,
} as const;

export const BUILDER_ZOOM_LIMITS = {
	minZoom: 0.2,
	maxZoom: 2,
} as const;
