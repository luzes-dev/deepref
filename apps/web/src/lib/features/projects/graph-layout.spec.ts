import { describe, expect, it } from 'vitest';
import {
	getGraphNodeSize,
	getGraphNodeSizeRange,
	getMinimumNodeDistance,
	getNodeSpacingGap
} from './graph-layout';

describe('graph layout math', () => {
	it('uses the base node size for isolated nodes', () => {
		expect(getGraphNodeSize(0)).toBe(4);
	});

	it('uses the existing square-root node size scale', () => {
		expect(getGraphNodeSize(9)).toBe(13);
	});

	it('caps node size at the maximum graph size', () => {
		expect(getGraphNodeSize(10_000)).toBe(24);
	});

	it('shrinks minimum and maximum node size as graph size grows', () => {
		const smallGraph = getGraphNodeSizeRange(100);
		const largeGraph = getGraphNodeSizeRange(1_000);

		expect(smallGraph.min).toBe(4);
		expect(smallGraph.max).toBe(24);
		expect(largeGraph.min).toBeLessThan(smallGraph.min);
		expect(largeGraph.max).toBeLessThan(smallGraph.max);
		expect(largeGraph.min).toBeGreaterThanOrEqual(1.5);
		expect(largeGraph.max).toBeGreaterThanOrEqual(8);
	});

	it('uses smaller sizes for the same degree in larger graphs', () => {
		expect(getGraphNodeSize(9, 1_000)).toBeLessThan(getGraphNodeSize(9, 100));
	});

	it('grows spacing monotonically and nonlinearly with larger node size', () => {
		const smallGap = getNodeSpacingGap(4);
		const mediumGap = getNodeSpacingGap(12);
		const largeGap = getNodeSpacingGap(20);

		expect(smallGap).toBe(2);
		expect(mediumGap).toBeGreaterThan(smallGap);
		expect(largeGap).toBeGreaterThan(mediumGap);
		expect(largeGap - mediumGap).toBeGreaterThan(mediumGap - smallGap);
	});

	it('uses the larger node to calculate symmetric minimum distance', () => {
		const sourceFirst = getMinimumNodeDistance(6, 18);
		const targetFirst = getMinimumNodeDistance(18, 6);
		const expected = 6 + 18 + getNodeSpacingGap(18);

		expect(sourceFirst).toBe(targetFirst);
		expect(sourceFirst).toBe(expected);
	});

	it('supports a grid size large enough for the largest current node distance', () => {
		const maxNodeSize = getGraphNodeSize(10_000);
		const gridSize = Math.ceil(getMinimumNodeDistance(maxNodeSize, maxNodeSize));

		expect(gridSize).toBeGreaterThanOrEqual(getMinimumNodeDistance(maxNodeSize, maxNodeSize));
	});
});
