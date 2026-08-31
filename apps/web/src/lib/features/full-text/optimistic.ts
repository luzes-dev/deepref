export type FullTextQueueItem = {
	report_id: string;
	full_text_status: string;
	revision: number;
};

export type FullTextQueueCache = {
	pages: Array<{
		data: { items: FullTextQueueItem[]; next_cursor?: string | null };
		[key: string]: unknown;
	}>;
	pageParams: unknown[];
};

export function applyFullTextState(
	cache: FullTextQueueCache | undefined,
	state: Pick<FullTextQueueItem, 'report_id' | 'full_text_status' | 'revision'>
): FullTextQueueCache | undefined {
	if (!cache) return cache;
	return {
		...cache,
		pages: cache.pages.map((page) => ({
			...page,
			data: {
				...page.data,
				items: page.data.items.map((item) =>
					item.report_id === state.report_id ? { ...item, ...state } : item
				)
			}
		}))
	};
}
