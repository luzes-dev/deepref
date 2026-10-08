import { describe, expect, it } from 'vitest';
import { IMPORT_FALLBACK_MAX_DEPTH } from './constants';
import { parseRememberedDepth, resolveImportDepth } from './ingestion-depth';

describe('resolveImportDepth', () => {
	it('prefers a remembered choice over the Settings default', () => {
		expect(resolveImportDepth({ remembered: 3, settingsDefault: 0 })).toEqual({
			depth: 3,
			source: 'remembered'
		});
	});

	it('treats a remembered 0 as a real choice, not a missing one', () => {
		expect(resolveImportDepth({ remembered: 0, settingsDefault: 2 })).toEqual({
			depth: 0,
			source: 'remembered'
		});
	});

	it.each([0, 1, 2, 7])(
		'starts from the Settings default %i when nothing is remembered',
		(value) => {
			expect(resolveImportDepth({ settingsDefault: value })).toEqual({
				depth: value,
				source: 'settings'
			});
		}
	);

	it('falls back to the built-in depth when Settings have not loaded', () => {
		expect(resolveImportDepth({})).toEqual({
			depth: IMPORT_FALLBACK_MAX_DEPTH,
			source: 'fallback'
		});
	});

	it.each([Number.NaN, -1, 1.5, Number.POSITIVE_INFINITY])(
		'ignores an invalid remembered value %s and then uses Settings',
		(remembered) => {
			expect(resolveImportDepth({ remembered, settingsDefault: 0 })).toEqual({
				depth: 0,
				source: 'settings'
			});
		}
	);

	it.each([Number.NaN, -2, 0.5])('ignores an invalid Settings value %s', (settingsDefault) => {
		expect(resolveImportDepth({ settingsDefault })).toEqual({
			depth: IMPORT_FALLBACK_MAX_DEPTH,
			source: 'fallback'
		});
	});

	it('keeps a Settings default above the form cap, because the user set it', () => {
		expect(resolveImportDepth({ settingsDefault: 6 })).toEqual({
			depth: 6,
			source: 'settings'
		});
	});
});

describe('parseRememberedDepth', () => {
	it.each([
		[null, undefined],
		['', undefined],
		['   ', undefined],
		['abc', undefined],
		['-1', undefined],
		['1.5', undefined],
		['Infinity', undefined],
		['0', 0],
		['2', 2],
		['4', 4]
	])('reads %j as %j', (raw, expected) => {
		expect(parseRememberedDepth(raw)).toBe(expected);
	});
});
