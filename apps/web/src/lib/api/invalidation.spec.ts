import { describe, expect, it } from 'vitest';
import { getGetScreeningQueueQueryKey } from './generated/review/review.js';
import { screeningKeys } from '../features/screening/api.js';

const generatedSources = import.meta.glob<string>('./generated/**/*.ts', {
	query: '?raw',
	import: 'default',
	eager: true
});

describe('generated project invalidation', () => {
	it('never emits a bare projects prefix that refetches unrelated projects', () => {
		for (const [path, source] of Object.entries(generatedSources)) {
			expect(source, path).not.toMatch(/\.startsWith\(['"`]\/api\/projects\/['"`]\)/);
		}
	});

	it('proposal decisions target the generated title/abstract queue for the mutation project', () => {
		const ai = generatedSources['./generated/ai/ai.ts'];
		expect(ai).toContain('getGetScreeningQueueQueryKey(variables.projectId)');
		expect(screeningKeys.queue('project-a')).toEqual(getGetScreeningQueueQueryKey('project-a'));
		expect(screeningKeys.queue('project-a')).not.toEqual(screeningKeys.queue('project-b'));
	});
});
