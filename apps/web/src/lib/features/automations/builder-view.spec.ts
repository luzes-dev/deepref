import { describe, expect, it } from 'vitest';
import { freeSpot, headerHelp, unavailableReason, type BuilderControlState } from './builder-view';

const idle: BuilderControlState = {
	readOnly: false,
	busy: false,
	conflict: false,
	blockCount: 2,
	published: true
};

describe('unavailableReason', () => {
	it('explains nothing when every control can be used', () => {
		for (const action of ['save', 'check', 'test', 'publish', 'switch'] as const) {
			expect(unavailableReason(action, idle)).toBeNull();
		}
	});

	it('says why a switch is off until the automation is published', () => {
		expect(unavailableReason('switch', { ...idle, published: false })).toBe(
			'Publish first. You can turn it on after that.'
		);
	});

	it('tells people to add a block before a test', () => {
		expect(unavailableReason('test', { ...idle, blockCount: 0 })).toBe(
			'Add a block first. A test needs a starting point.'
		);
		expect(unavailableReason('publish', { ...idle, blockCount: 0 })).toBeNull();
	});

	it('explains that a past run cannot be edited, but the switch still works', () => {
		for (const action of ['save', 'check', 'test', 'publish'] as const) {
			expect(unavailableReason(action, { ...idle, readOnly: true })).toMatch(/past run/);
		}
		expect(unavailableReason('switch', { ...idle, readOnly: true })).toBeNull();
	});

	it('blocks saving and publishing after a conflict, with a way out', () => {
		for (const action of ['save', 'publish'] as const) {
			expect(unavailableReason(action, { ...idle, conflict: true })).toMatch(
				/Load the latest version/
			);
		}
		expect(unavailableReason('check', { ...idle, conflict: true })).toBeNull();
	});

	it('waits while another request is running', () => {
		for (const action of ['save', 'check', 'test', 'publish', 'switch'] as const) {
			expect(unavailableReason(action, { ...idle, busy: true })).toBe(
				'Wait for the current step to finish.'
			);
		}
	});
});

describe('headerHelp', () => {
	it('shows nothing when everything is available', () => {
		expect(headerHelp(idle, false)).toBeNull();
	});

	it('explains the switch while the automation is not published', () => {
		expect(headerHelp({ ...idle, published: false }, false)).toBe(
			'Publish first. You can turn it on after that.'
		);
	});

	it('points a manual automation to Run instead of the switch', () => {
		expect(headerHelp({ ...idle, published: false }, true)).toBe(
			'Publish first. Then you can run it by hand.'
		);
		expect(headerHelp(idle, true)).toBeNull();
	});

	it('explains a past run first, and stays quiet while something is running', () => {
		expect(headerHelp({ ...idle, readOnly: true }, false)).toMatch(/past run/);
		expect(headerHelp({ ...idle, busy: true, published: false }, false)).toBeNull();
	});
});

describe('freeSpot', () => {
	const size = { width: 256, height: 120 };

	it('keeps the wanted position when nothing is in the way', () => {
		expect(freeSpot({ x: 300, y: 200 }, [], size)).toEqual({ x: 300, y: 200 });
	});

	it('moves a new block off a block that is already there, to the nearest free spot', () => {
		const taken = [{ x: 280, y: 180, width: 256, height: 120 }];
		const spot = freeSpot({ x: 300, y: 200 }, taken, size);
		expect(spot).not.toEqual({ x: 300, y: 200 });
		const overlaps =
			spot.x < taken[0].x + taken[0].width + 20 &&
			spot.x + size.width + 20 > taken[0].x &&
			spot.y < taken[0].y + taken[0].height + 20 &&
			spot.y + size.height + 20 > taken[0].y;
		expect(overlaps).toBe(false);
		// Nearest free spot is one step away, not a long way off.
		expect(Math.hypot(spot.x - 300, spot.y - 200)).toBeLessThanOrEqual(120);
	});

	it('keeps the position when the area is crowded beyond the search radius', () => {
		const taken = [];
		for (let i = -10; i <= 10; i += 1)
			for (let j = -10; j <= 10; j += 1)
				taken.push({ x: 300 + i * 40, y: 200 + j * 40, width: 300, height: 300 });
		expect(freeSpot({ x: 300, y: 200 }, taken, size)).toEqual({ x: 300, y: 200 });
	});
});
