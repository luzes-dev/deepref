import { fc, test } from '@fast-check/vitest';
import { expect } from 'vitest';
import {
	fullTextSearchParams,
	parseFullTextUrl,
	type FullTextFilter,
	type FullTextUrlState
} from './url';

const fullTextFilters: FullTextFilter[] = ['all', 'missing', 'available', 'failed'];
const nonEmptyText = fc.string({ minLength: 1, maxLength: 128 });

test.prop({
	filter: fc.constantFrom(...fullTextFilters),
	report: nonEmptyText,
	page: fc.integer({ min: 1, max: 100_000 }),
	block: nonEmptyText
})('round-trips every valid full-text URL state', (state) => {
	const validState: FullTextUrlState = state;

	expect(parseFullTextUrl(fullTextSearchParams(validState))).toEqual(validState);
});
