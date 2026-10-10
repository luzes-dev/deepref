import { afterEach, describe, expect, it, vi } from 'vitest';
import { currentReviewerId, saveReviewerId } from './reviewer';

afterEach(() => vi.unstubAllGlobals());

describe('local reviewer attribution', () => {
	it('has a server-safe default without browser state', () => {
		expect(currentReviewerId()).toBe('local-user');
		expect(() => saveReviewerId('reviewer-a')).toThrow('requires a browser');
	});

	it('keeps IDs in the current browser session', () => {
		const entries = new Map<string, string>();
		vi.stubGlobal('window', {
			sessionStorage: {
				getItem: (key: string) => entries.get(key) ?? null,
				setItem: (key: string, value: string) => entries.set(key, value)
			}
		});
		expect(currentReviewerId()).toBe('local-user');
		saveReviewerId(' reviewer-a ');
		expect(currentReviewerId()).toBe('reviewer-a');
		saveReviewerId('reviewer-b');
		expect(currentReviewerId()).toBe('reviewer-b');
	});

	it('refuses empty, unsafe header and oversized IDs', () => {
		for (const id of ['', '   ', 'reviewer\nother', 'two people', 'é', 'x'.repeat(129)]) {
			expect(() => saveReviewerId(id)).toThrow('printable ASCII');
		}
	});

	it('falls back safely when browser storage cannot be read', () => {
		vi.stubGlobal('window', {
			sessionStorage: {
				getItem: () => {
					throw new Error('storage blocked');
				}
			}
		});
		expect(currentReviewerId()).toBe('local-user');
	});
});
