import { describe, expect, it } from 'vitest';
import { canRequestAiSuggestions } from './availability';

describe('canRequestAiSuggestions', () => {
	it('waits until status resolves', () => {
		expect(canRequestAiSuggestions({ data: undefined, isError: false })).toBe(false);
	});

	it('enables suggestions only when status reports availability', () => {
		expect(
			canRequestAiSuggestions({
				data: { data: { suggestions_available: true } },
				isError: false
			})
		).toBe(true);
		expect(
			canRequestAiSuggestions({
				data: { data: { suggestions_available: false } },
				isError: false
			})
		).toBe(false);
	});

	it('fails open when status lookup errors, including with stale unavailable data', () => {
		expect(canRequestAiSuggestions({ data: undefined, isError: true })).toBe(true);
		expect(
			canRequestAiSuggestions({
				data: { data: { suggestions_available: false } },
				isError: true
			})
		).toBe(true);
	});
});
