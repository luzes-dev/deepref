import { describe, expect, it } from 'vitest';
import {
	displayDedupeTitle,
	formatDedupeJson,
	formatDedupeScore,
	formatDedupeYear
} from './formatters';

describe('deduplication display formatters', () => {
	it('keeps comparison fields readable when source metadata is missing', () => {
		expect(displayDedupeTitle('  ')).toBe('Untitled source record');
		expect(formatDedupeYear(null)).toBe('Year unknown');
		expect(formatDedupeJson({})).toBe('None');
	});

	it('formats explainable score components without losing labels', () => {
		expect(formatDedupeScore(0.826)).toBe('83%');
		expect(formatDedupeJson({ doi: '10.5555/example', pmid: 123 })).toBe(
			'doi: 10.5555/example · pmid: 123'
		);
	});
});
