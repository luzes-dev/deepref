import { fc, test } from '@fast-check/vitest';
import { expect } from 'vitest';
import { normalizedBboxToPixels } from './bbox';

test.prop({
	x: fc.double({ min: 0, max: 0.999, noNaN: true, noDefaultInfinity: true }),
	y: fc.double({ min: 0, max: 0.999, noNaN: true, noDefaultInfinity: true }),
	width: fc.double({ min: 0.001, max: 2, noNaN: true, noDefaultInfinity: true }),
	height: fc.double({ min: 0.001, max: 2, noNaN: true, noDefaultInfinity: true }),
	pageWidth: fc.integer({ min: 1, max: 4_000 }),
	pageHeight: fc.integer({ min: 1, max: 4_000 })
})(
	'keeps valid normalized boxes inside page pixel bounds',
	({ x, y, width, height, pageWidth, pageHeight }) => {
		const result = normalizedBboxToPixels({ x, y, width, height }, pageWidth, pageHeight);

		if (!result) throw new Error('a positive normalized region should produce a pixel box');

		const tolerance = Math.max(pageWidth, pageHeight) * Number.EPSILON * 8;
		expect(result.x).toBeGreaterThanOrEqual(0);
		expect(result.y).toBeGreaterThanOrEqual(0);
		expect(result.width).toBeGreaterThan(0);
		expect(result.height).toBeGreaterThan(0);
		expect(result.x + result.width).toBeLessThanOrEqual(pageWidth + tolerance);
		expect(result.y + result.height).toBeLessThanOrEqual(pageHeight + tolerance);
	}
);
