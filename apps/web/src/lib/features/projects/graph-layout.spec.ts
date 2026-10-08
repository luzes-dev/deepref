import { describe, expect, it } from 'vitest';
import {
	fitBBoxWithLabels,
	getGraphNodeSize,
	getGraphNodeSizeRange,
	getMinimumNodeDistance,
	getNodeSpacingGap,
	layoutIsolatedNodes
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

describe('graph framing', () => {
	it('stacks isolated nodes in a column below the connected cluster', () => {
		const connected = [
			{ x: 0, y: 0 },
			{ x: 20, y: 10 }
		];
		const placed = layoutIsolatedNodes(connected, 3);
		expect(placed).toHaveLength(3);
		expect(new Set(placed.map((point) => point.x))).toEqual(new Set([0]));
		// Below the cluster on screen: sigma's y axis points up.
		expect(placed[0].y).toBeLessThan(0);
		expect(placed[1].y).toBeLessThan(placed[0].y);
	});

	it('places an all-isolated graph at a finite origin-based column', () => {
		const placed = layoutIsolatedNodes([], 14);
		expect(placed.every((point) => Number.isFinite(point.x) && Number.isFinite(point.y))).toBe(
			true
		);
		expect(placed[12].x).toBeGreaterThan(placed[0].x);
	});

	it('reserves label room on the right of the fitted bounding box', () => {
		const bbox = fitBBoxWithLabels(
			[
				{ x: 0, y: 0 },
				{ x: 100, y: 50 }
			],
			{ width: 800, height: 600 },
			{ stagePadding: 24, labelWidth: 180, nodePadding: 16 }
		);
		expect(bbox).toBeDefined();
		expect(bbox!.x[0]).toBeLessThan(0);
		expect(bbox!.x[1] - 100).toBeGreaterThan(0 - bbox!.x[0]);
	});

	it('reserves room at the top for floating canvas controls', () => {
		const nodes = [
			{ x: 0, y: 0 },
			{ x: 100, y: 100 }
		];
		const base = { stagePadding: 24, labelWidth: 120, nodePadding: 16 };
		const plain = fitBBoxWithLabels(nodes, { width: 800, height: 600 }, base);
		const inset = fitBBoxWithLabels(
			nodes,
			{ width: 800, height: 600 },
			{ ...base, topInset: 56 }
		);
		expect(inset!.y[1] - inset!.y[0]).toBeGreaterThan(plain!.y[1] - plain!.y[0]);
	});

	it('returns nothing for an empty graph or an unusably small viewport', () => {
		const options = { stagePadding: 24, labelWidth: 180, nodePadding: 16 };
		expect(fitBBoxWithLabels([], { width: 800, height: 600 }, options)).toBeUndefined();
		expect(
			fitBBoxWithLabels([{ x: 0, y: 0 }], { width: 100, height: 600 }, options)
		).toBeUndefined();
	});
});
