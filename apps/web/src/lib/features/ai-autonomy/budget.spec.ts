import { describe, expect, it } from 'vitest';
import { isAiPaused, parseBudgetInput } from './budget';

describe('parseBudgetInput', () => {
	it('accepts amounts from zero up to the limit', () => {
		expect(parseBudgetInput('0')).toEqual({ ok: true, amount: 0 });
		expect(parseBudgetInput(' 12.5 ')).toEqual({ ok: true, amount: 12.5 });
		expect(parseBudgetInput('100000')).toEqual({ ok: true, amount: 100000 });
	});

	it('rejects blank, negative, non-numeric and oversized amounts', () => {
		for (const raw of ['', '   ', '-1', 'abc', 'Infinity', '100001']) {
			const result = parseBudgetInput(raw);
			expect(result.ok, raw).toBe(false);
			if (!result.ok) {
				expect(result.message).toBe('Enter an amount between 0 and 100,000 dollars.');
			}
		}
	});
});

describe('isAiPaused', () => {
	it('is true only for a zero budget', () => {
		expect(isAiPaused(0)).toBe(true);
		expect(isAiPaused(0.5)).toBe(false);
		expect(isAiPaused(5)).toBe(false);
	});
});
