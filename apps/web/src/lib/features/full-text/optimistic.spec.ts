import { describe, expect, it } from 'vitest';
import { applyFullTextState } from './optimistic';

const cache = {
	pages: [
		{
			data: {
				next_cursor: null,
				items: [{ report_id: 'r1', full_text_status: 'unscreened', revision: 3 }]
			}
		}
	],
	pageParams: [undefined]
};

describe('full-text optimistic cache', () => {
	it('updates and can be restored from a snapshot', () => {
		const changed = applyFullTextState(cache, {
			report_id: 'r1',
			full_text_status: 'include',
			revision: 4
		});
		expect(changed?.pages[0].data.items[0]).toMatchObject({
			full_text_status: 'include',
			revision: 4
		});
		expect(cache.pages[0].data.items[0].full_text_status).toBe('unscreened');
	});

	it('applies the authoritative conflict state and revision', () => {
		const changed = applyFullTextState(cache, {
			report_id: 'r1',
			full_text_status: 'exclude',
			revision: 9
		});
		expect(changed?.pages[0].data.items[0]).toMatchObject({
			full_text_status: 'exclude',
			revision: 9
		});
	});
});
