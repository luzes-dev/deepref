import { describe, expect, it } from 'vitest';
import { parseAppraisalLocation, updateAppraisalLocation } from './url';

describe('appraisal URL state', () => {
	it('preserves report and definition selection across URL updates', () => {
		const current = new URLSearchParams('tab=appraisal&report=report-1');
		const next = updateAppraisalLocation(current, {
			definitionId: 'deepref-rct-generic',
			definitionVersion: 1
		});

		expect(parseAppraisalLocation(next)).toEqual({
			reportId: 'report-1',
			definitionId: 'deepref-rct-generic',
			definitionVersion: 1
		});
		expect(next.get('tab')).toBe('appraisal');
	});

	it('rejects invalid definition versions and supports explicit clearing', () => {
		const current = new URLSearchParams('report=report-1&definition_version=0');
		const next = updateAppraisalLocation(current, {
			reportId: '',
			definitionId: '',
			definitionVersion: 0
		});

		expect(parseAppraisalLocation(next)).toEqual({});
	});
});
