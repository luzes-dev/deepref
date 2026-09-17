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
