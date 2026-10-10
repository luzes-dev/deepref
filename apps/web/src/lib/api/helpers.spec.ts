import { QueryClient } from '@tanstack/svelte-query';
import type { MutationFunctionContext } from '@tanstack/svelte-query';
import { describe, expect, it } from 'vitest';
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
		const queryClient = new QueryClient({
			defaultOptions: { queries: { staleTime: Infinity } }
		});
		const targetedKeys = [
			getListIngestionsQueryKey(),
			getGetIngestionQueryKey('ingestion-1'),
			getListIngestionItemsQueryKey('ingestion-1')
		];
		const unaffectedKeys = [
			['/api/ingestions/ingestion-2'],
			['/api/ingestions/ingestion-2/items']
		];
		for (const queryKey of [...targetedKeys, ...unaffectedKeys]) {
			queryClient.setQueryData(queryKey, {});
		}
		const isStale = (queryKey: readonly unknown[]) =>
			queryClient.getQueryCache().find({ queryKey, exact: true })?.isStale() ?? false;
		const options = getCancelIngestionMutationOptions(queryClient);

		for (const queryKey of targetedKeys) expect(isStale(queryKey)).toBe(false);
		for (const queryKey of unaffectedKeys) expect(isStale(queryKey)).toBe(false);

		await options.onSuccess?.(
			{ data: undefined, status: 202, headers: new Headers() },
			{ ingestionId: 'ingestion-1' },
			undefined,
			{} as MutationFunctionContext
		);

		for (const queryKey of targetedKeys) expect(isStale(queryKey)).toBe(true);
		for (const queryKey of unaffectedKeys) expect(isStale(queryKey)).toBe(false);
	});
});
