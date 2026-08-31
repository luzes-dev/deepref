const fullTextFilters = ['all', 'missing', 'available', 'failed'] as const;

export type FullTextFilter = (typeof fullTextFilters)[number];

export type FullTextUrlState = {
	filter: FullTextFilter;
	report: string | null;
	page: number | null;
	block: string | null;
};

const defaultFullTextUrlState: FullTextUrlState = {
	filter: 'all',
	report: null,
	page: null,
	block: null
};

function oneOf<T extends readonly string[]>(value: string | null, values: T, fallback: T[number]) {
	return value && (values as readonly string[]).includes(value) ? (value as T[number]) : fallback;
}

function positivePage(value: string | null): number | null {
	if (!value) return null;
	const page = Number(value);
	return Number.isSafeInteger(page) && page > 0 ? page : null;
}

export function parseFullTextUrl(searchParams: URLSearchParams): FullTextUrlState {
	return {
		filter: oneOf(searchParams.get('filter'), fullTextFilters, defaultFullTextUrlState.filter),
		report: searchParams.get('report') || null,
		page: positivePage(searchParams.get('page')),
		block: searchParams.get('block') || null
	};
}

export function fullTextSearchParams(state: FullTextUrlState): URLSearchParams {
	const params = new URLSearchParams();
	if (state.filter !== defaultFullTextUrlState.filter) params.set('filter', state.filter);
	if (state.report) params.set('report', state.report);
	if (state.page) params.set('page', String(state.page));
	if (state.block) params.set('block', state.block);
	return params;
}

export function fullTextUrlString(state: FullTextUrlState): string {
	const query = fullTextSearchParams(state).toString();
	return query ? `?${query}` : '';
}
