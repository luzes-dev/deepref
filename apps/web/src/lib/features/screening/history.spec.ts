import { describe, expect, it } from 'vitest';
import { describeHistoryItem } from './history';

const base = {
	event_kind: 'decision',
	stage: 'title_abstract',
	decision: 'include',
	actor_kind: 'user',
	created_at: '2026-10-06T20:39:00',
	previous_title_abstract_status: 'unscreened',
	result_title_abstract_status: 'include',
	previous_full_text_status: 'not_required',
	result_full_text_status: 'not_required'
};

describe('describeHistoryItem', () => {
	it('summarizes a decision in plain language without the unchanged stage', () => {
		const item = describeHistoryItem(base);
		expect(item.headline).toBe('Included at title & abstract');
		expect(item.meta).toBe('Oct 6, 20:39 · you');
		expect(item.change).toBeNull();
	});

	it('mentions the other stage only when it changed', () => {
		const item = describeHistoryItem({
			...base,
			result_full_text_status: 'unscreened'
		});
		expect(item.change).toBe('Full text: not required → unscreened');
	});

	it('describes undo events', () => {
		const item = describeHistoryItem({ ...base, event_kind: 'undo', stage: 'full_text' });
		expect(item.headline).toBe('Undid the full text decision');
	});
});
