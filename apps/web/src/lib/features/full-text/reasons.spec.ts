import { describe, expect, it } from 'vitest';
import { sortExclusionReasons } from './reasons';

const reason = (code: string, label: string) => ({ code, label });

describe('sortExclusionReasons', () => {
	it('orders by relevance with other last', () => {
		const sorted = sortExclusionReasons([
			reason('other', 'Other'),
			reason('conference_abstract_only', 'Conference abstract only'),
			reason('wrong_design', 'Wrong study design'),
			reason('wrong_comparator_outcome', 'Wrong comparator/outcome'),
			reason('duplicate', 'Duplicate'),
			reason('wrong_intervention', 'Wrong intervention or exposure'),
			reason('wrong_population', 'Wrong population')
		]);
		expect(sorted.map((item) => item.code)).toEqual([
			'wrong_population',
			'wrong_intervention',
			'wrong_comparator_outcome',
			'wrong_design',
			'conference_abstract_only',
			'duplicate',
			'other'
		]);
	});
});
