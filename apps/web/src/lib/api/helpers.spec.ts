import { QueryClient } from '@tanstack/svelte-query';
import type { MutationFunctionContext } from '@tanstack/svelte-query';
import { describe, expect, it } from 'vitest';
import {
	getCancelAcquisitionMutationOptions,
	getGetAcquisitionQueryKey,
	getListAcquisitionsQueryKey,
	getListAcquisitionItemsQueryKey
} from './generated/acquisitions/acquisitions';
import { getGetProjectReportQueryKey } from './generated/reports/reports';
import { shouldPollIngestion } from './helpers';

describe('generated query keys', () => {
	it('include every path parameter', () => {
		expect(getGetAcquisitionQueryKey('project-1', 'acquisition-1')).toEqual([
			'/api/projects/project-1/acquisitions/acquisition-1'
		]);
		expect(getListAcquisitionItemsQueryKey('project-1', 'acquisition-1')).toEqual([
			'/api/projects/project-1/acquisitions/acquisition-1/items'
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
	it('invalidates only the project acquisition list and cancelled run detail/items', async () => {
		const queryClient = new QueryClient({
			defaultOptions: { queries: { staleTime: Infinity } }
		});
		const targetedKeys = [
			getListAcquisitionsQueryKey('project-1'),
			getGetAcquisitionQueryKey('project-1', 'acquisition-1'),
			getListAcquisitionItemsQueryKey('project-1', 'acquisition-1')
		];
		const unaffectedKeys = [
			getListAcquisitionsQueryKey('project-2'),
			getGetAcquisitionQueryKey('project-1', 'acquisition-2'),
			getListAcquisitionItemsQueryKey('project-1', 'acquisition-2')
		];
		for (const queryKey of [...targetedKeys, ...unaffectedKeys]) {
			queryClient.setQueryData(queryKey, {});
		}
		const isStale = (queryKey: readonly unknown[]) =>
			queryClient.getQueryCache().find({ queryKey, exact: true })?.isStale() ?? false;
		const options = getCancelAcquisitionMutationOptions(queryClient);

		for (const queryKey of targetedKeys) expect(isStale(queryKey)).toBe(false);
		for (const queryKey of unaffectedKeys) expect(isStale(queryKey)).toBe(false);

		await options.onSuccess?.(
			{ data: undefined, status: 202, headers: new Headers() },
			{ projectId: 'project-1', acquisitionId: 'acquisition-1' },
			undefined,
			{} as MutationFunctionContext
		);

		for (const queryKey of targetedKeys) expect(isStale(queryKey)).toBe(true);
		for (const queryKey of unaffectedKeys) expect(isStale(queryKey)).toBe(false);
	});
});
