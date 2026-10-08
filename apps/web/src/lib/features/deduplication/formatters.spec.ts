import { describe, expect, it } from 'vitest';
import {
	dedupeRunTotals,
	displayDedupeTitle,
	formatDedupeJson,
	formatDedupeRunSummary,
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

	it('counts fuzzy auto-merges as merges, not as possible duplicates', () => {
		const run = { processed: 5, proposals_created: 2, auto_linked: 1, auto_accepted: 1 };
		expect(dedupeRunTotals(run)).toEqual({ processed: 5, possible: 1, merged: 2 });
		expect(formatDedupeRunSummary(run, 'just now')).toBe(
			'Checked 5 records · 1 possible duplicate · 2 merged automatically · just now'
		);
	});

	it('describes a quiet run without merges and tolerates older API payloads', () => {
		expect(
			formatDedupeRunSummary(
				{ processed: 8, proposals_created: 0, auto_linked: 0 },
				'just now'
			)
		).toBe('Checked 8 records · 0 possible duplicates · just now');
		expect(
			dedupeRunTotals({ processed: 1, proposals_created: 0, auto_linked: 0 }).possible
		).toBe(0);
	});
});
