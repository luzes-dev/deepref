import { describe, expect, it } from 'vitest';
import type { ScreeningQueueItemDto } from '$lib/api/generated/models';
import {
	applyOptimisticStatusChange,
	findQueueLocation,
	type ScreeningQueueCache
} from './optimistic';

function item(reportId: string, status: string, revision = 0): ScreeningQueueItemDto {
	return {
		report_id: reportId,
		title: `Report ${reportId}`,
		abstract_text: `Abstract ${reportId}`,
		doi: null,
		publication_year: 2024,
		title_abstract_status: status,
		full_text_status: status === 'include' ? 'unscreened' : 'not_required',
		final_status: status === 'include' ? 'pending_full_text' : status,
		revision
	};
}

function cache(): ScreeningQueueCache {
	return {
		pages: [
			{
				data: {
					items: [item('a', 'unscreened'), item('b', 'unscreened')],
					next_cursor: 'page-2',
					progress: {
						total: 4,
						screened: 0,
						unscreened: 4,
						included: 0,
						excluded: 0,
						maybe: 0
					},
					sort: 'created_asc',
					status: 'all',
					total: 4
				}
			},
			{
				data: {
					items: [item('c', 'unscreened'), item('d', 'unscreened')],
					next_cursor: null,
					progress: {
						total: 4,
						screened: 0,
						unscreened: 4,
						included: 0,
						excluded: 0,
						maybe: 0
					},
					sort: 'created_asc',
					status: 'all',
					total: 4
				}
			}
		],
		pageParams: [undefined, 'page-2']
	};
}

describe('screening optimistic cache transforms', () => {
	it('removes a decided item from the unscreened queue and updates progress once', () => {
		const result = applyOptimisticStatusChange(cache(), {
			reportId: 'a',
			fromStatus: 'unscreened',
			toStatus: 'include',
			revision: 1,
			filterStatus: 'unscreened'
		});

		expect(result?.pages[0].data.items.map((row) => row.report_id)).toEqual(['b']);
		expect(result?.pages[0].data.total).toBe(3);
		expect(result?.pages[0].data.progress).toMatchObject({
			screened: 1,
			unscreened: 3,
			included: 1
		});
		expect(result?.pages[1].data.progress).toMatchObject({ screened: 0, unscreened: 4 });
	});

	it('keeps status changes in the all queue', () => {
		const result = applyOptimisticStatusChange(cache(), {
			reportId: 'a',
			fromStatus: 'unscreened',
			toStatus: 'maybe',
			revision: 2,
			filterStatus: 'all'
		});

		expect(result?.pages[0].data.items[0]).toMatchObject({
			report_id: 'a',
			title_abstract_status: 'maybe',
			revision: 2
		});
		expect(result?.pages[0].data.total).toBe(4);
	});

	it('removes an included row when an include-filtered decision changes to exclude', () => {
		const includeCache = cache();
		includeCache.pages[0].data.status = 'include';
		includeCache.pages[0].data.items = [item('a', 'include')];
		includeCache.pages[0].data.total = 1;
		includeCache.pages[0].data.progress = {
			total: 4,
			screened: 1,
			unscreened: 3,
			included: 1,
			excluded: 0,
			maybe: 0
		};
		const result = applyOptimisticStatusChange(includeCache, {
			reportId: 'a',
			fromStatus: 'include',
			toStatus: 'exclude',
			revision: 2,
			filterStatus: 'include'
		});

		expect(result?.pages[0].data.items).toEqual([]);
		expect(result?.pages[0].data.total).toBe(0);
		expect(result?.pages[0].data.progress).toMatchObject({
			included: 0,
			excluded: 1,
			screened: 1
		});
	});

	it('reinserts a newly matching row at its saved location', () => {
		const filtered = cache();
		filtered.pages[0].data.status = 'include';
		filtered.pages[0].data.items = [item('b', 'include')];
		filtered.pages[0].data.total = 1;
		const prior = item('a', 'unscreened');
		const result = applyOptimisticStatusChange(filtered, {
			reportId: 'a',
			fromStatus: 'unscreened',
			toStatus: 'include',
			revision: 1,
			filterStatus: 'include',
			priorItem: prior,
			priorLocation: { pageIndex: 0, itemIndex: 0 }
		});

		expect(result?.pages[0].data.items.map((row) => row.report_id)).toEqual(['a', 'b']);
		expect(result?.pages[0].data.items[0].title_abstract_status).toBe('include');
		expect(result?.pages[0].data.total).toBe(2);
	});

	it('updates progress when the changed row is on a later loaded page', () => {
		const before = cache();
		const result = applyOptimisticStatusChange(before, {
			reportId: 'c',
			fromStatus: 'unscreened',
			toStatus: 'exclude',
			revision: 1,
			filterStatus: 'all'
		});

		expect(result?.pages[0].data.progress).toMatchObject({
			screened: 1,
			unscreened: 3,
			excluded: 1
		});
		expect(result?.pages[1].data.items[0]).toMatchObject({
			title_abstract_status: 'exclude',
			revision: 1
		});
		expect(before.pages[1].data.items[0].title_abstract_status).toBe('unscreened');
	});

	it('reinserts a post-decision undo into its previous stable location and leaves rollback snapshots intact', () => {
		const before = cache();
		const decided = applyOptimisticStatusChange(before, {
			reportId: 'a',
			fromStatus: 'unscreened',
			toStatus: 'include',
			revision: 1,
			filterStatus: 'unscreened',
			priorItem: item('a', 'unscreened'),
			priorLocation: { pageIndex: 0, itemIndex: 0 }
		});
		const restored = applyOptimisticStatusChange(decided, {
			reportId: 'a',
			fromStatus: 'include',
			toStatus: 'unscreened',
			revision: 2,
			filterStatus: 'unscreened',
			priorItem: item('a', 'include', 1),
			priorLocation: { pageIndex: 0, itemIndex: 0 }
		});

		expect(restored?.pages[0].data.items[0].report_id).toBe('a');
		expect(findQueueLocation(restored, 'a')).toEqual({ pageIndex: 0, itemIndex: 0 });
		expect(before.pages[0].data.items[0].title_abstract_status).toBe('unscreened');
	});
});
