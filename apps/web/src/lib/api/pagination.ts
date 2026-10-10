import type { listIngestionsResponseSuccess } from '#lib/api/generated/ingestions/ingestions.js';
import { getListIngestionsUrl } from '#lib/api/generated/ingestions/ingestions.js';
import { getListProjectsUrl } from '#lib/api/generated/projects/projects.js';
import type { listProjectsResponseSuccess } from '#lib/api/generated/projects/projects.js';
import { customFetch } from './custom-fetch';

export const API_PAGE_LIMIT = 50;

export function cursorUrl(url: string, cursor?: string, limit = API_PAGE_LIMIT): string {
	const search = new URLSearchParams({ limit: String(limit) });
	if (cursor) search.set('cursor', cursor);
	return `${url}?${search}`;
}

export function fetchProjectsPage(
	cursor: string | undefined,
	signal?: AbortSignal
): Promise<listProjectsResponseSuccess> {
	return customFetch(cursorUrl(getListProjectsUrl(), cursor), { method: 'GET', signal });
}

export function fetchIngestionsPage(
	cursor: string | undefined,
	signal?: AbortSignal
): Promise<listIngestionsResponseSuccess> {
	return customFetch(cursorUrl(getListIngestionsUrl(), cursor), { method: 'GET', signal });
}
