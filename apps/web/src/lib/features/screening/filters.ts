const screeningModes = ['focus', 'table'] as const;
const screeningStatuses = ['unscreened', 'include', 'exclude', 'maybe', 'all'] as const;
const screeningSorts = [
	'created_asc',
	'created_desc',
	'title_asc',
	'title_desc',
	'year_asc',
	'year_desc'
] as const;

export type ScreeningMode = (typeof screeningModes)[number];
export type ScreeningStatus = (typeof screeningStatuses)[number];
export type ScreeningSort = (typeof screeningSorts)[number];

export type ScreeningUrlState = {
	mode: ScreeningMode;
	status: ScreeningStatus;
	search: string;
	sort: ScreeningSort;
	report: string | null;
};

export const defaultScreeningUrlState: ScreeningUrlState = {
	mode: 'focus',
	status: 'unscreened',
	search: '',
	sort: 'created_asc',
	report: null
};

function oneOf<T extends readonly string[]>(value: string | null, values: T, fallback: T[number]) {
	return value && (values as readonly string[]).includes(value) ? (value as T[number]) : fallback;
}

export function parseScreeningUrl(searchParams: URLSearchParams): ScreeningUrlState {
	const search = searchParams.get('search')?.trim() ?? '';
	return {
		mode: oneOf(searchParams.get('mode'), screeningModes, defaultScreeningUrlState.mode),
		status: oneOf(
			searchParams.get('status'),
			screeningStatuses,
			defaultScreeningUrlState.status
		),
		search,
		sort: oneOf(searchParams.get('sort'), screeningSorts, defaultScreeningUrlState.sort),
		report: searchParams.get('report') || null
	};
}

export function screeningUrlSearchParams(state: ScreeningUrlState): URLSearchParams {
	const params = new URLSearchParams();
	if (state.mode !== defaultScreeningUrlState.mode) params.set('mode', state.mode);
	if (state.status !== defaultScreeningUrlState.status) params.set('status', state.status);
	if (state.search) params.set('search', state.search);
	if (state.sort !== defaultScreeningUrlState.sort) params.set('sort', state.sort);
	if (state.report) params.set('report', state.report);
	return params;
}

export function screeningUrlString(state: ScreeningUrlState): string {
	const query = screeningUrlSearchParams(state).toString();
	return query ? `?${query}` : '';
}
