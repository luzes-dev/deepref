const BASE_NODE_SIZE = 4;
const MAX_NODE_SIZE = 24;
const BASE_GRAPH_SIZE_FOR_NODE_SCALE = 100;
const MIN_SCALED_NODE_SIZE = 1.5;
const MIN_SCALED_MAX_NODE_SIZE = 8;
const BASE_NODE_GAP = 2;
const MAX_EXPONENTIAL_GAP = 36;
const SPACING_EXPONENT_BASE = 1.12;
const SPACING_EXPONENT_SCALE = 3;

export function getGraphNodeSizeRange(graphSize: number): {
	min: number;
	max: number;
	scale: number;
} {
	const scale =
		graphSize > BASE_GRAPH_SIZE_FOR_NODE_SCALE
			? Math.sqrt(BASE_GRAPH_SIZE_FOR_NODE_SCALE / graphSize)
			: 1;
	const min = Math.max(MIN_SCALED_NODE_SIZE, BASE_NODE_SIZE * scale);
	const max = Math.max(MIN_SCALED_MAX_NODE_SIZE, MAX_NODE_SIZE * scale);

	return { min, max: Math.max(min, max), scale };
}

export function getGraphNodeSize(
	degree: number,
	graphSize = BASE_GRAPH_SIZE_FOR_NODE_SCALE
): number {
	const { min, max, scale } = getGraphNodeSizeRange(graphSize);

	return Math.min(max, min + Math.sqrt(degree) * 3 * scale);
}

export function getNodeSpacingGap(largerSize: number): number {
	const scaledSize = Math.max(0, largerSize - BASE_NODE_SIZE);
	const exponentialGap =
		(Math.pow(SPACING_EXPONENT_BASE, scaledSize) - 1) * SPACING_EXPONENT_SCALE;

	return BASE_NODE_GAP + Math.min(MAX_EXPONENTIAL_GAP, exponentialGap);
}

export function getMinimumNodeDistance(sourceSize: number, targetSize: number): number {
	const largerSize = Math.max(sourceSize, targetSize);

	return sourceSize + targetSize + getNodeSpacingGap(largerSize);
}

export type GraphPoint = { x: number; y: number };
export type GraphBBox = { x: [number, number]; y: [number, number] };

const ISOLATED_MIN_SPAN = 10;
const ISOLATED_ROW_FRACTION = 0.08;
const ISOLATED_MAX_ROWS = 12;

/**
 * Positions for nodes without links: a tidy column (or a few columns) below the
 * connected cluster, aligned with its left edge, instead of wherever the force
 * layout drifts them. Single columns keep right-hand labels from colliding.
 */
export function layoutIsolatedNodes(connected: readonly GraphPoint[], count: number): GraphPoint[] {
	if (count <= 0) return [];
	const xs = connected.map((point) => point.x);
	const ys = connected.map((point) => point.y);
	const hasCluster = connected.length > 0;
	const minX = hasCluster ? Math.min(...xs) : 0;
	const minY = hasCluster ? Math.min(...ys) : 0;
	const maxY = hasCluster ? Math.max(...ys) : 0;
	const span = hasCluster
		? Math.max(ISOLATED_MIN_SPAN, Math.max(...xs) - minX, maxY - Math.min(...ys))
		: ISOLATED_MIN_SPAN * 2;
	const rowGap = span * ISOLATED_ROW_FRACTION;
	const columnGap = span * 0.7;
	// Sigma's y axis points up, so "below the cluster" on screen means smaller y.
	const startY = hasCluster ? minY - rowGap * 3 : 0;

	return Array.from({ length: count }, (_, index) => ({
		x: minX + Math.floor(index / ISOLATED_MAX_ROWS) * columnGap,
		y: startY - (index % ISOLATED_MAX_ROWS) * rowGap
	}));
}

/**
 * A bounding box for the renderer's auto-fit that reserves room (in screen
 * pixels) for the label drawn to the right of the rightmost nodes and for the
 * node radius on every side, so nothing is clipped at the viewport edge.
 */
export function fitBBoxWithLabels(
	nodes: readonly GraphPoint[],
	viewport: { width: number; height: number },
	options: { stagePadding: number; labelWidth: number; nodePadding: number; topInset?: number }
): GraphBBox | undefined {
	if (nodes.length === 0) return undefined;
	const minX = Math.min(...nodes.map((node) => node.x));
	const maxX = Math.max(...nodes.map((node) => node.x));
	const minY = Math.min(...nodes.map((node) => node.y));
	const maxY = Math.max(...nodes.map((node) => node.y));
	const dataW = Math.max(maxX - minX, 1e-6);
	const dataH = Math.max(maxY - minY, 1e-6);
	const innerW = viewport.width - 2 * options.stagePadding;
	const innerH = viewport.height - 2 * options.stagePadding;
	const availW = innerW - options.labelWidth - 2 * options.nodePadding;
	// Floating controls sit over the top of the canvas; keep nodes and labels out from under them.
	const topInset = options.topInset ?? 0;
	const availH = innerH - 2 * options.nodePadding - topInset;
	if (availW <= 0 || availH <= 0) return undefined;
	const scale = Math.min(availW / dataW, availH / dataH);
	// A single point (or a degenerate line) is not scale-limited; keep it centred.
	const safeScale = Number.isFinite(scale) ? Math.min(scale, 1e6) : 1;
	const left = options.nodePadding / safeScale;
	const right = (options.nodePadding + options.labelWidth) / safeScale;
	const vertical = options.nodePadding / safeScale;
	return {
		x: [minX - left, maxX + right],
		y: [minY - vertical, maxY + vertical + topInset / safeScale]
	};
}
