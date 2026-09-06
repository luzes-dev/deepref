import { fc, test } from '@fast-check/vitest';
import { expect } from 'vitest';
import {
	defaultScreeningUrlState,
	parseScreeningUrl,
	screeningUrlSearchParams,
	type ScreeningMode,
	type ScreeningSort,
	type ScreeningStatus,
	type ScreeningUrlState
} from './filters';

const modes: ScreeningMode[] = ['focus', 'table'];
const statuses: ScreeningStatus[] = ['unscreened', 'include', 'exclude', 'maybe', 'all'];
const sorts: ScreeningSort[] = [
	'created_asc',
	'created_desc',
	'title_asc',
	'title_desc',
	'year_asc',
	'year_desc'
];

test.prop({
	mode: fc.constantFrom(...modes),
	status: fc.constantFrom(...statuses),
	search: fc.string({ maxLength: 128 }),
	sort: fc.constantFrom(...sorts),
	report: fc.oneof(fc.constant(null), fc.string({ minLength: 1, maxLength: 128 }))
})('round-trips valid filters and normalizes the search term', (state) => {
	const validState: ScreeningUrlState = state;
	const parsed = parseScreeningUrl(screeningUrlSearchParams(validState));

	expect(parsed).toEqual({
		...validState,
		search: validState.search.trim()
	});

	if (
		validState.mode === defaultScreeningUrlState.mode &&
		validState.status === defaultScreeningUrlState.status &&
		validState.sort === defaultScreeningUrlState.sort &&
		!validState.search.trim() &&
		!validState.report
	) {
		expect(screeningUrlSearchParams(validState).toString()).toBe('');
	}
});
