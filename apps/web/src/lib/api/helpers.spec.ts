import { QueryClient } from '@tanstack/svelte-query';
import type { MutationFunctionContext } from '@tanstack/svelte-query';
import { describe, expect, it, vi } from 'vitest';
import {
	getCancelIngestionMutationOptions,
	getGetIngestionQueryKey,
	getListIngestionsQueryKey,
	getListIngestionItemsQueryKey
} from './generated/ingestions/ingestions';
import { getGetProjectReportQueryKey } from './generated/reports/reports';
import { shouldPollIngestion } from './helpers';

describe('generated query keys', () => {
	it('include every path parameter', () => {
		expect(getGetIngestionQueryKey('ingestion-1')).toEqual(['/api/ingestions/ingestion-1']);
		expect(getListIngestionItemsQueryKey('ingestion-1')).toEqual([
			'/api/ingestions/ingestion-1/items'
		]);
		expect(getGetProjectReportQueryKey('project-1', 'report-1')).toEqual([
			'/api/projects/project-1/reports/report-1'
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
