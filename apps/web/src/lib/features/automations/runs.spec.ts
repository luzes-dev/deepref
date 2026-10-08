import { describe, expect, it } from 'vitest';
import { reviewCountsLine, runLabel } from './runs';

describe('runLabel', () => {
	it('calls a test run a test run, not "Test (test)"', () => {
		expect(runLabel({ trigger: 'test', test_mode: true })).toBe('Test run');
		expect(runLabel({ trigger: 'manual', test_mode: true })).toBe('Test run');
	});

	it('names what started a real run', () => {
		expect(runLabel({ trigger: 'manual', test_mode: false })).toBe('Run by hand');
		expect(runLabel({ trigger: 'webhook', test_mode: false })).toBe('Web address call');
		expect(runLabel({ trigger: 'schedule', test_mode: false })).toBe('Schedule');
		expect(runLabel({ trigger: 'something_new', test_mode: false })).toBe(
			'Started automatically'
		);
	});
});

describe('reviewCountsLine', () => {
	it('shows the per-verdict counts, leaving out zeros', () => {
		expect(reviewCountsLine({ included: 9, excluded: 0, unsure: 3 })).toBe(
			'9 would include · 3 not sure'
		);
		expect(reviewCountsLine({ included: 1, excluded: 4, unsure: 0 })).toBe(
			'1 would include · 4 would exclude'
		);
	});

	it('says so when the review had nothing to judge', () => {
		expect(reviewCountsLine({ included: 0, excluded: 0, unsure: 0 })).toBe(
			'The AI review had no records to judge'
		);
	});

	it('is absent for a run that used no AI review', () => {
		expect(reviewCountsLine(null)).toBeNull();
		expect(reviewCountsLine(undefined)).toBeNull();
	});
});
