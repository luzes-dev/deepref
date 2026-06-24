import { QueryClient } from '@tanstack/svelte-query';
import type { MutationFunctionContext } from '@tanstack/svelte-query';
import { describe, expect, it, vi } from 'vitest';
import {
	getCancelIngestionMutationOptions,
	getGetIngestionQueryKey,
	getListIngestionsQueryKey,
	getListIngestionItemsQueryKey
} from './generated/ingestions/ingestions';
import { getGetProjectArticleQueryKey } from './generated/articles/articles';
import { shouldPollIngestion } from './helpers';

describe('generated query keys', () => {
	it('include every path parameter', () => {
		expect(getGetIngestionQueryKey('ingestion-1')).toEqual([
			'http://localhost:8080/ingestions/ingestion-1'
		]);
		expect(getListIngestionItemsQueryKey('ingestion-1')).toEqual([
			'http://localhost:8080/ingestions/ingestion-1/items'
		]);
		expect(getGetProjectArticleQueryKey('project-1', 'doi-key')).toEqual([
			'http://localhost:8080/projects/project-1/articles/doi-key'
		]);
	});
});

describe('ingestion polling', () => {
	it.each([undefined, 'queued', 'running'])('polls status %s every two seconds', (status) => {
		expect(shouldPollIngestion(status)).toBe(2_000);
	});

	it.each(['completed', 'failed', 'cancelled'])('stops polling terminal status %s', (status) => {
		expect(shouldPollIngestion(status)).toBe(false);
	});
});

describe('generated mutation invalidation', () => {
	it('invalidates ingestion list, detail, and items after cancellation', async () => {
		const queryClient = new QueryClient();
		const invalidate = vi.spyOn(queryClient, 'invalidateQueries').mockResolvedValue(undefined);
		const options = getCancelIngestionMutationOptions(queryClient);

		await options.onSuccess?.(
			{ data: undefined, status: 202, headers: new Headers() },
			{ ingestionId: 'ingestion-1' },
			undefined,
			{} as MutationFunctionContext
		);

		expect(invalidate).toHaveBeenCalledWith({ queryKey: getListIngestionsQueryKey() });
		expect(invalidate).toHaveBeenCalledWith({
			queryKey: getGetIngestionQueryKey('ingestion-1')
		});
		expect(invalidate).toHaveBeenCalledWith({
			queryKey: getListIngestionItemsQueryKey('ingestion-1')
		});
	});
});
