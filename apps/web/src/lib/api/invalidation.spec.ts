import { QueryClient } from '@tanstack/svelte-query';
import { describe, expect, it } from 'vitest';
import {
	getDecideAiProposalMutationOptions,
	getGetAiProposalQueryKey,
	type DecideAiProposalMutationResult
} from './generated/ai/ai.js';
import { getGetScreeningQueueQueryKey } from './generated/review/review.js';

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
		expect(getGetScreeningQueueQueryKey('project-a')).toEqual([
			'/api/projects/project-a/screening'
		]);
		expect(getGetScreeningQueueQueryKey('project-a')).not.toEqual(
			getGetScreeningQueueQueryKey('project-b')
		);
	});
});

describe('proposal decision cache invalidation', () => {
	it('refreshes the mutation project without invalidating another project', async () => {
		const queryClient = new QueryClient();
		const queueA = getGetScreeningQueueQueryKey('project-a');
		const queueB = getGetScreeningQueueQueryKey('project-b');
		const proposalA = getGetAiProposalQueryKey('project-a', 'proposal-a');
		const proposalB = getGetAiProposalQueryKey('project-b', 'proposal-b');
		const response: DecideAiProposalMutationResult = {
			status: 200,
			headers: new Headers(),
			data: {
				applied_revision: 2,
				proposal: {
					id: 'proposal-a',
					project_id: 'project-a',
					entity_type: 'report',
					entity_id: 'report-a',
					task_kind: 'title_abstract_screening',
					operation: 'screening',
					authority_tier: 'advisory',
					status: 'accepted',
					created_at: '2026-10-11T00:00:00Z',
					input_hash: 'input',
					model: 'test-model',
					model_run_id: 'model-run',
					model_version: '1',
					prompt_hash: 'prompt',
					prompt_version: '1',
					provider: 'test-provider',
					schema_hash: 'schema',
					schema_version: '1',
					payload: {
						kind: 'screening',
						criteria: [],
						expected_revision: 1,
						protocol_version_id: 'protocol-a',
						report_id: 'report-a',
						stage: 'title_abstract',
						suggested_decision: { kind: 'include' },
						task_kind: 'title_abstract_screening',
						uncertainties: []
					}
				}
			}
		};
		try {
			queryClient.setQueryData(queueA, []);
			queryClient.setQueryData(queueB, []);
			queryClient.setQueryData(proposalA, response.data.proposal);
			queryClient.setQueryData(proposalB, {
				...response.data.proposal,
				id: 'proposal-b',
				project_id: 'project-b'
			});
			const options = getDecideAiProposalMutationOptions(queryClient);
			await options.onSuccess?.(
				response,
				{
					projectId: 'project-a',
					proposalId: 'proposal-a',
					data: { decision: 'accept', reason: 'Eligible study' }
				},
				undefined,
				{ client: queryClient, meta: undefined, mutationKey: options.mutationKey }
			);
			expect(queryClient.getQueryState(queueA)?.isInvalidated).toBe(true);
			expect(queryClient.getQueryState(proposalA)?.isInvalidated).toBe(true);
			expect(queryClient.getQueryState(queueB)?.isInvalidated).toBe(false);
			expect(queryClient.getQueryState(proposalB)?.isInvalidated).toBe(false);
		} finally {
			queryClient.clear();
		}
	});
});
