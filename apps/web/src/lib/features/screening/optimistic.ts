import type {
	ScreeningQueueDto,
	ScreeningQueueItemDto,
	ScreeningProgressDto,
	ScreeningStateDto
} from '$lib/api/generated/models';

export type ScreeningQueuePage = { data: ScreeningQueueDto };

export type ScreeningQueueCache = {
	pages: ScreeningQueuePage[];
	pageParams: unknown[];
};

export type QueueLocation = {
	pageIndex: number;
	itemIndex: number;
};

export type OptimisticStatusChange = {
	reportId: string;
	fromStatus: string;
	toStatus: string;
	revision: number;
	filterStatus: string;
	priorItem?: ScreeningQueueItemDto;
	priorLocation?: QueueLocation | null;
};

export function findQueueLocation(
	cache: ScreeningQueueCache | undefined,
	reportId: string
): QueueLocation | null {
	if (!cache) return null;
	for (const [pageIndex, page] of cache.pages.entries()) {
		const itemIndex = page.data.items.findIndex((item) => item.report_id === reportId);
		if (itemIndex >= 0) return { pageIndex, itemIndex };
	}
	return null;
}

export function findQueueItem(
	cache: ScreeningQueueCache | undefined,
	reportId: string
): ScreeningQueueItemDto | undefined {
	const location = findQueueLocation(cache, reportId);
	return location ? cache?.pages[location.pageIndex].data.items[location.itemIndex] : undefined;
}

function matchesFilter(status: string, filterStatus: string): boolean {
	return filterStatus === 'all' || status === filterStatus;
}

function progressAfter(
	progress: ScreeningProgressDto,
	fromStatus: string,
	toStatus: string
): ScreeningProgressDto {
	if (fromStatus === toStatus) return progress;
	const next = { ...progress };
	const counts: Record<string, keyof ScreeningProgressDto> = {
		include: 'included',
		exclude: 'excluded',
		maybe: 'maybe'
	};
	if (fromStatus === 'unscreened' && toStatus !== 'unscreened') {
		next.screened += 1;
		next.unscreened = Math.max(0, next.unscreened - 1);
	}
	if (fromStatus !== 'unscreened' && toStatus === 'unscreened') {
		next.screened = Math.max(0, next.screened - 1);
		next.unscreened += 1;
	}
	if (counts[fromStatus]) next[counts[fromStatus]] = Math.max(0, next[counts[fromStatus]] - 1);
	if (counts[toStatus]) next[counts[toStatus]] += 1;
	return next;
}

function totalDelta(fromStatus: string, toStatus: string, filterStatus: string): number {
	const before = matchesFilter(fromStatus, filterStatus);
	const after = matchesFilter(toStatus, filterStatus);
	return Number(after) - Number(before);
}

/**
 * Applies only the local projection change. The caller owns rollback and the
 * server mutation; this function never mutates the input cache.
 */
export function applyOptimisticStatusChange(
	cache: ScreeningQueueCache | undefined,
	action: OptimisticStatusChange
): ScreeningQueueCache | undefined {
	if (!cache) return cache;
	const location = findQueueLocation(cache, action.reportId) ?? action.priorLocation ?? null;
	const existing = findQueueItem(cache, action.reportId) ?? action.priorItem;
	const shouldRemain = matchesFilter(action.toStatus, action.filterStatus);
	const shouldHaveBeen = matchesFilter(action.fromStatus, action.filterStatus);
	const nextItem = existing
		? { ...existing, title_abstract_status: action.toStatus, revision: action.revision }
		: undefined;
	const pages = cache.pages.map((page, pageIndex) => {
		const itemIndex = page.data.items.findIndex((item) => item.report_id === action.reportId);
		let items = page.data.items;
		if (itemIndex >= 0) {
			if (shouldRemain && nextItem) {
				items = [...items];
				items[itemIndex] = nextItem;
			} else {
				items = items.filter((item) => item.report_id !== action.reportId);
			}
		} else if (
			!shouldHaveBeen &&
			shouldRemain &&
			nextItem &&
			location?.pageIndex === pageIndex
		) {
			items = [...items];
			items.splice(Math.min(location.itemIndex, items.length), 0, nextItem);
		}
		const delta = totalDelta(action.fromStatus, action.toStatus, action.filterStatus);
		return {
			...page,
			data: {
				...page.data,
				items,
				total: pageIndex === 0 ? Math.max(0, page.data.total + delta) : page.data.total,
				progress:
					pageIndex === 0
						? progressAfter(page.data.progress, action.fromStatus, action.toStatus)
						: page.data.progress
			}
		};
	});
	return { ...cache, pages };
}

export function applyServerStateToQueue(
	cache: ScreeningQueueCache | undefined,
	state: ScreeningStateDto,
	filterStatus: string,
	fallbackItem?: ScreeningQueueItemDto,
	priorLocation?: QueueLocation | null
): ScreeningQueueCache | undefined {
	const existing = findQueueItem(cache, state.report_id) ?? fallbackItem;
	return applyOptimisticStatusChange(cache, {
		reportId: state.report_id,
		fromStatus: existing?.title_abstract_status ?? 'unscreened',
		toStatus: state.title_abstract_status,
		revision: state.revision,
		filterStatus,
		priorItem: existing,
		priorLocation
	});
}
