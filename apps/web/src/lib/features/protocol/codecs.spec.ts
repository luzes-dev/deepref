import { describe, expect, it } from 'vitest';
import {
	FRAMEWORK_FIELDS,
	duplicateCustomKeys,
	frameworkFieldsForKind,
	humanizeKey,
	isCriterionDimension,
	isFrameworkKind
} from './codecs';

describe('protocol codecs', () => {
	it('keeps only known fields for structured frameworks', () => {
		expect(
			frameworkFieldsForKind('pico', {
				population: 'Adults',
				outcome: 'Sleep',
				unexpected: 'ignored'
			})
		).toEqual({
			population: 'Adults',
			intervention: '',
			comparator: '',
			outcome: 'Sleep'
		});
		expect(FRAMEWORK_FIELDS.spider).toContain('research_type');
	});

	it('preserves arbitrary custom fields', () => {
		expect(frameworkFieldsForKind('custom', { lens: 'qualitative', scope: 'global' })).toEqual({
			lens: 'qualitative',
			scope: 'global'
		});
	});

	it('detects duplicate custom field keys after trimming', () => {
		expect(
			duplicateCustomKeys([
				{ key: ' scope ' },
				{ key: 'scope' },
				{ key: 'other' },
				{ key: '' }
			])
		).toEqual(['scope']);
	});

	it('narrows framework and criterion values at the boundary', () => {
		expect(isFrameworkKind('picos')).toBe(true);
		expect(isFrameworkKind('unknown')).toBe(false);
		expect(isCriterionDimension('outcome')).toBe(true);
		expect(isCriterionDimension('unknown')).toBe(false);
	});

	it('humanizes structured field names', () => {
		expect(humanizeKey('title_abstract')).toBe('Title Abstract');
	});
});
