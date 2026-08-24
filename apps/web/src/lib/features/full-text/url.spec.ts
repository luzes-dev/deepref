import { describe, expect, it } from 'vitest';
import { fullTextSearchParams, fullTextUrlString, parseFullTextUrl } from './url';

describe('full-text URL state', () => {
	it('round trips report, filter, page, and block', () => {
		const state = {
			filter: 'available' as const,
			report: 'r1',
			page: 4,
			block: 'b7'
		};
		expect(parseFullTextUrl(fullTextSearchParams(state))).toEqual(state);
		expect(fullTextUrlString(state)).toBe('?filter=available&report=r1&page=4&block=b7');
	});

	it('rejects invalid or unsafe page values', () => {
		expect(parseFullTextUrl(new URLSearchParams('page=0'))).toMatchObject({ page: null });
		expect(parseFullTextUrl(new URLSearchParams('page=-1'))).toMatchObject({ page: null });
		expect(parseFullTextUrl(new URLSearchParams('page=abc'))).toMatchObject({ page: null });
	});
});
