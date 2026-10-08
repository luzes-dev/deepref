import { describe, expect, it } from 'vitest';
import { agreementSummary, emptyConflictCopy, decisionLabel, rationaleLines } from './conflicts';

describe('conflicts helpers', () => {
	it('labels decisions in plain language', () => {
		expect(decisionLabel('include')).toBe('Include');
		expect(decisionLabel(null)).toBe('No decision yet');
	});

	it('summarises agreement with and without kappa', () => {
		expect(agreementSummary({ compared: 0, agreed: 0 })).toBe('No decisions to compare yet.');
		expect(agreementSummary({ compared: 10, agreed: 8, kappa: 0.5 })).toContain('kappa) 0.50');
		expect(agreementSummary({ compared: 4, agreed: 3 })).toBe(
			'You and the AI agreed on 3 of 4 decisions (75%).'
		);
	});
});

describe('conflicts empty states and rationale', () => {
	it('never claims agreement when nothing was compared', () => {
		const none = emptyConflictCopy('conflict', 0);
		expect(none.title).toBe('No decisions to compare yet');
		expect(none.description).not.toMatch(/agree on every/);
		expect(emptyConflictCopy('conflict', 3).title).toBe('No conflicts');
		expect(emptyConflictCopy('waiting', 0).title).toBe('No AI opinions waiting for you');
	});

	it('splits a rationale into one line per criterion', () => {
		expect(
			rationaleLines('Population (meets): yes\n\n Outcome (unclear): not stated \n')
		).toEqual(['Population (meets): yes', 'Outcome (unclear): not stated']);
		expect(rationaleLines(null)).toEqual([]);
	});
});
