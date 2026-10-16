import type { listIngestionsResponseSuccess } from '#lib/api/generated/ingestions/ingestions.js';
import { getListIngestionsUrl } from '#lib/api/generated/ingestions/ingestions.js';
import { getListProjectsUrl } from '#lib/api/generated/projects/projects.js';
import type { listProjectsResponseSuccess } from '#lib/api/generated/projects/projects.js';
import { customFetch } from './custom-fetch';
import { createQuery } from '@tanstack/svelte-query';
import type { DocumentBlockDto } from './generated/models/index.js';
import {
	getListDocumentBlocksQueryKey,
	listDocumentBlocks,
	type listDocumentBlocksResponseSuccess
} from './generated/documents/documents.js';

export async function fetchAllDocumentBlocks(
	projectId: string,
	reportId: string,
	documentId: string,
	limit = 100,
	signal?: AbortSignal
) {
	const items: DocumentBlockDto[] = [];
	let cursor: string | undefined;
	let response: listDocumentBlocksResponseSuccess;
	do {
		response = await listDocumentBlocks(
			projectId,
			reportId,
			documentId,
			{ cursor, limit },
			{ signal }
		);
		items.push(...response.data.items);
		cursor = response.data.next_cursor ?? undefined;
	} while (cursor);
	return { ...response, data: items };
}

/** Collect bounded pages under one generated document key; never publish partial overlays. */
export function createAllDocumentBlocks(
	projectId: () => string,
	reportId: () => string,
	documentId: () => string,
	params: () => { limit?: number },
	options: () => { query: { enabled: boolean } }
) {
	return createQuery(() => ({
		queryKey: [
			...getListDocumentBlocksQueryKey(projectId(), reportId(), documentId(), params()),
			'all-pages'
		],
		enabled: options().query.enabled,
		queryFn: ({ signal }) =>
			fetchAllDocumentBlocks(
				projectId(),
				reportId(),
				documentId(),
				params().limit ?? 100,
				signal
			)
	}));
}

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
