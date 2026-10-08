import { describe, expect, it } from 'vitest';
import { flowTotals, reconciliationSentence } from './flow';

const base = {
	identified_records: 11,
	manually_created_reports: 0,
	duplicates_removed: 2,
	unresolved_records: 2,
	screened_records: 7
};

describe('PRISMA flow totals', () => {
	it('reconciles identified records against removed, awaiting and screened counts', () => {
		const totals = flowTotals(base);
		expect(totals).toEqual({
			identified: 11,
			duplicatesRemoved: 2,
			awaitingDuplicateCheck: 2,
			screened: 7,
			reconciles: true
		});
		expect(reconciliationSentence(totals)).toBe(
			'11 identified = 2 duplicates removed + 2 awaiting duplicate check + 7 screened'
		);
	});

	it('adds manually created reports to the identified total', () => {
		const totals = flowTotals({
			...base,
			identified_records: 9,
			manually_created_reports: 2,
			unresolved_records: 0,
			duplicates_removed: 2,
			screened_records: 9
		});
		expect(totals.identified).toBe(11);
		expect(totals.reconciles).toBe(true);
		expect(reconciliationSentence(totals)).toBe(
			'11 identified = 2 duplicates removed + 9 screened'
		);
	});

	it('does not invent an awaiting term once every record is resolved', () => {
		const totals = flowTotals({ ...base, unresolved_records: 0, screened_records: 9 });
		expect(totals.reconciles).toBe(true);
		expect(reconciliationSentence(totals)).not.toContain('awaiting');
	});

	it('says so when the counts do not add up', () => {
		const totals = flowTotals({ ...base, screened_records: 6 });
		expect(totals.reconciles).toBe(false);
		expect(reconciliationSentence(totals)).toMatch(/do not add up/);
	});
});
