import { describe, expect, it } from 'vitest';
import { normalizedBboxToPixels } from './bbox';

describe('normalizedBboxToPixels', () => {
	it('maps normalized coordinates to page pixels', () => {
		expect(
			normalizedBboxToPixels({ x: 0.1, y: 0.2, width: 0.5, height: 0.25 }, 600, 800)
		).toEqual({
			x: 60,
			y: 160,
			width: 300,
			height: 200
		});
	});

	it('clips hostile coordinates and ignores empty regions', () => {
		expect(normalizedBboxToPixels({ x: -1, y: 0, width: 2, height: 0.5 }, 100, 100)).toEqual({
			x: 0,
			y: 0,
			width: 100,
			height: 50
		});
		expect(normalizedBboxToPixels({ x: 1, y: 1, width: 0, height: 0 }, 100, 100)).toBeNull();
	});
});
