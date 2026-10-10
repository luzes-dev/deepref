import { describe, expect, it } from 'vitest';
import type { AiActivityDto } from '#lib/api/generated/models/index.js';
import { groupActivity, undoableCount } from './activity';

function entry(id: string, batch: string | null, undoable = true, undone = false): AiActivityDto {
	return {
		id,
		actor_type: 'ai',
		actor_label: 'model',
		task: 'extraction',
		task_label: 'Data extraction',
		action: 'x',
		summary: 'did something',
		affected: [],
		undoable,
		batch_id: batch,
		ai_run_id: null,
		proposal_id: null,
		model: null,
		prompt_version: null,
		evidence: [],
		created_at: '2026-10-07T00:00:00Z',
		undone_at: undone ? '2026-10-07T01:00:00Z' : null,
		undone_by: null
	};
}

describe('groupActivity', () => {
	it('groups entries of the same batch and keeps single entries alone', () => {
		const groups = groupActivity([entry('a', 'b1'), entry('b', null), entry('c', 'b1')]);
		expect(groups.map((group) => group.kind)).toEqual(['batch', 'single']);
		expect(groups[0].kind === 'batch' && groups[0].entries.map((item) => item.id)).toEqual([
			'a',
			'c'
		]);
	});

	it('treats a batch of one as a single entry', () => {
		expect(groupActivity([entry('a', 'b1')])[0].kind).toBe('single');
	});

	it('counts only entries that can still be undone', () => {
		expect(
			undoableCount([entry('a', null), entry('b', null, false), entry('c', null, true, true)])
		).toBe(1);
	});
});
