import { describe, expect, it } from 'vitest';
import { parseStudyLocation, updateStudyLocation } from './url';

describe('study URL state', () => {
	it('round-trips selected study and report without losing unrelated params', () => {
		const original = new URLSearchParams('tab=history&study=study-1&report=report-1');
		const next = updateStudyLocation(original, { studyId: 'study-2' });

		expect(parseStudyLocation(next)).toEqual({ studyId: 'study-2', reportId: 'report-1' });
		expect(next.get('tab')).toBe('history');
	});

	it('removes a selection explicitly', () => {
		const next = updateStudyLocation(new URLSearchParams('study=study-1'), { studyId: '' });
		expect(parseStudyLocation(next)).toEqual({});
	});
});
