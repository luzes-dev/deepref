import { describe, expect, it } from 'vitest';
import {
	defaultScreeningUrlState,
	parseScreeningUrl,
	screeningUrlSearchParams,
	screeningUrlString
} from './filters';

describe('screening URL filters', () => {
	it('round-trips shareable focus/table filters', () => {
		const state = {
			mode: 'table' as const,
			status: 'maybe' as const,
			search: '  climate policy  ',
			sort: 'title_desc' as const,
			report: 'report-123'
		};
		const parsed = parseScreeningUrl(screeningUrlSearchParams(state));

		expect(parsed).toEqual({ ...state, search: 'climate policy' });
		expect(screeningUrlString(parsed)).toBe(
			'?mode=table&status=maybe&search=climate+policy&sort=title_desc&report=report-123'
		);
	});

	it('rejects unknown values and omits defaults', () => {
		const parsed = parseScreeningUrl(
			new URLSearchParams('mode=unknown&status=wat&sort=wat&report=')
		);

		expect(parsed).toEqual(defaultScreeningUrlState);
		expect(screeningUrlString(parsed)).toBe('');
	});
});
